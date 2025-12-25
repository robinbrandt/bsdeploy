use crate::{config, remote};
use anyhow::Result;
use sha2::{Sha256, Digest};
use std::collections::BTreeMap;
use indicatif::ProgressBar;

pub fn get_image_hash(config: &config::Config, base_version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(base_version.as_bytes());
    
    // Hash packages (sorted)
    let mut pkgs = config.packages.clone();
    pkgs.sort();
    for pkg in pkgs {
        hasher.update(pkg.as_bytes());
        hasher.update(b";");
    }

    // Hash Mise (sorted keys)
    let mise_btree: BTreeMap<_, _> = config.mise.iter().collect();
    for (tool, version) in mise_btree {
        hasher.update(tool.as_bytes());
        hasher.update(b":");
        hasher.update(version.as_bytes());
        hasher.update(b";");
    }

    if let Some(user) = &config.user {
        hasher.update(b"user:");
        hasher.update(user.as_bytes());
    }

    hex::encode(hasher.finalize())
}

fn maybe_doas(cmd: &str, doas: bool) -> String {
    if doas {
        format!("doas {}", cmd)
    } else {
        cmd.to_string()
    }
}

pub fn ensure_image(config: &config::Config, host: &str, base_version: &str, spinner: &ProgressBar) -> Result<String> {
    let hash = get_image_hash(config, base_version);
    let short_hash = &hash[..12];
    let image_path = format!("/usr/local/bsdeploy/images/{}", short_hash);
    let cmd_prefix = if config.doas { "doas " } else { "" };

    // Check if valid image exists (by checking ZFS snapshot)
    if let Ok(Some(images_parent_ds)) = remote::get_zfs_dataset(host, "/usr/local/bsdeploy/images") {
        let image_ds = format!("{}/{}", images_parent_ds, short_hash);
        let snap_name = format!("{}@base", image_ds);
        
        if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", snap_name)).is_ok() {
            spinner.set_message(format!("[{}] Using existing image {}", host, short_hash));
            return Ok(image_path);
        }
        
        // If dataset exists but no snapshot, it's a failed build. Cleanup.
        if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", image_ds)).is_ok() {
            spinner.set_message(format!("[{}] Cleaning up incomplete image build...", host));
            remote::run(host, &format!("{}zfs destroy -r {}", cmd_prefix, image_ds))?;
        }
    } else {
        // Non-ZFS fallback check
        if remote::run(host, &format!("test -d {}/usr/local", image_path)).is_ok() {
             return Ok(image_path);
        }
    }

    spinner.set_message(format!("[{}] Building image {} (in-place)...", host, short_hash));

    // 1. Create Image Dataset & Populate Base
    let base_dir = format!("/usr/local/bsdeploy/base/{}", base_version);
    let mut zfs_cloned_base = false;

    if let Ok(Some(images_parent_ds)) = remote::get_zfs_dataset(host, "/usr/local/bsdeploy/images") {
         let image_ds = format!("{}/{}", images_parent_ds, short_hash);
         
         // Check if Base has @clean snapshot
         let mut base_snap = String::new();
         if let Ok(Some(base_ds)) = remote::get_zfs_dataset(host, &base_dir) {
             let snap = format!("{}@clean", base_ds);
             if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", snap)).is_ok() {
                 base_snap = snap;
             }
         }

         if !base_snap.is_empty() {
             // THIN IMAGE: Clone from Base
             spinner.set_message(format!("[{}] Image: Cloning base system (Thin)...", host));
             remote::run(host, &maybe_doas(&format!("zfs clone -o mountpoint={} {} {}", image_path, base_snap, image_ds), config.doas))?;
             zfs_cloned_base = true;
         } else {
             // THICK IMAGE: Create empty + Rsync
             remote::run(host, &maybe_doas(&format!("zfs create -o mountpoint={} {}", image_path, image_ds), config.doas))?;
         }
    } else {
         remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, image_path))?;
    }

    if !zfs_cloned_base {
        spinner.set_message(format!("[{}] Image: Populating base system (Thick)...", host));
        remote::run(host, &format!("{}rsync -a --exclude 'var/empty' {}/ {}", cmd_prefix, base_dir, image_path))?;
        // Fix var/empty permissions (rsync excludes it due to flags)
        remote::run(host, &format!("{}mkdir -p {}/var/empty", cmd_prefix, image_path))?;
        remote::run(host, &format!("{}chmod 555 {}/var/empty", cmd_prefix, image_path))?;
    }

    // 2. Setup Build Jail (Directly on image_path)
    let build_jail_name = format!("build-{}", short_hash);
    
    // Mount devfs
    remote::run(host, &format!("{}mount -t devfs devfs {}/dev", cmd_prefix, image_path))?;
    // Copy resolv.conf
    remote::run(host, &format!("{}cp /etc/resolv.conf {}/etc/", cmd_prefix, image_path))?;

    // Start Jail
    let start_cmd = format!(
        "{}jail -c name={} path={} host.hostname={} ip4=inherit allow.raw_sockets=1 persist",
        cmd_prefix, build_jail_name, image_path, build_jail_name
    );
    
    if let Err(e) = remote::run(host, &start_cmd) {
        remote::run(host, &format!("{}umount {}/dev", cmd_prefix, image_path)).ok();
        return Err(e);
    }

    // 3. Install Packages & Configuration
    let res = (|| -> Result<()> {
        spinner.set_message(format!("[{}] Image: Installing packages...", host));
        remote::run(host, &format!("{}pkg -j {} install -y git bash", cmd_prefix, build_jail_name))?;
        if !config.packages.is_empty() {
            let pkgs = config.packages.join(" ");
            remote::run(host, &format!("{}pkg -j {} install -y {}", cmd_prefix, build_jail_name, pkgs))?;
        }

        // Create User
        if let Some(user) = &config.user {
            let check_user = format!("{}jexec {} id {}", cmd_prefix, build_jail_name, user);
            if remote::run(host, &check_user).is_err() {
                remote::run(host, &format!("{}jexec {} pw useradd -n {} -m -s /usr/local/bin/bash", cmd_prefix, build_jail_name, user))?;
            }
        }

        // Install Mise
        if !config.mise.is_empty() {
            spinner.set_message(format!("[{}] Image: Installing Mise and build dependencies...", host));
            remote::run(host, &format!("{}pkg -j {} install -y mise gmake gcc python3 pkgconf", cmd_prefix, build_jail_name))?;
            for (tool, version) in &config.mise {
                 spinner.set_message(format!("[{}] Image: Building {}@{}...", host, tool, version));
                 let cmd = format!("export CC=gcc CXX=g++ MAKE=gmake && mise use --global {}@{}", tool, version);
                 let exec_cmd = if let Some(user) = &config.user {
                     format!("{}jexec {} su - {} -c \"{}\"", cmd_prefix, build_jail_name, user, cmd.replace("\"", "\\\""))
                 } else {
                     format!("{}jexec {} bash -c '{}'", cmd_prefix, build_jail_name, cmd)
                 };
                 remote::run(host, &exec_cmd)?;
            }
        }
        
        // Cleanup pkg cache inside jail
        remote::run(host, &format!("{}pkg -j {} clean -y", cmd_prefix, build_jail_name))?;
        Ok(())
    })();

    // 4. Teardown Jail
    remote::run(host, &format!("{}jail -r {}", cmd_prefix, build_jail_name))?;
    remote::run(host, &format!("{}umount {}/dev", cmd_prefix, image_path))?;

    if let Err(e) = res {
        // If build failed, destroy the dataset so we don't leave broken state
        if let Ok(Some(images_parent_ds)) = remote::get_zfs_dataset(host, "/usr/local/bsdeploy/images") {
             let image_ds = format!("{}/{}", images_parent_ds, short_hash);
             remote::run(host, &format!("{}zfs destroy -r {}", cmd_prefix, image_ds)).ok();
        }
        return Err(e);
    }

    // 5. Snapshot
    if let Ok(Some(dataset)) = remote::get_zfs_dataset(host, &image_path) {
        spinner.set_message(format!("[{}] Image: Creating ZFS snapshot...", host));
        let snap_name = format!("{}@base", dataset);
        remote::run(host, &format!("{}zfs snapshot {}", cmd_prefix, snap_name))?;
    }

    Ok(image_path)
}
