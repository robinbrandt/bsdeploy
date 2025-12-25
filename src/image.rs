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

    // Check if image exists
    if remote::run(host, &format!("test -d {}/usr/local", image_path)).is_ok() {
        spinner.set_message(format!("[{}] Using existing image {}", host, short_hash));
        return Ok(image_path);
    }

    spinner.set_message(format!("[{}] Building image {} (this may take a while)...", host, short_hash));

    // Create a temporary build jail
    // We can reuse jail::create logic but we need to customize it heavily.
    // Let's manually do it to be precise.
    let build_jail_name = format!("build-{}", short_hash);
    let build_root = format!("/usr/local/bsdeploy/jails/{}", build_jail_name);
    
    // Cleanup previous failed build if any
    if remote::run(host, &format!("test -d {}", build_root)).is_ok() {
        spinner.set_message(format!("[{}] Cleaning up stale build environment...", host));
        // Stop jail if running
        remote::run(host, &format!("{}jail -r {}", cmd_prefix, build_jail_name)).ok();
        
        // Unmount everything under build_root
        // We grep mount points and unmount them
        let mount_check = format!("mount | grep '{}' | awk '{{print $3}}'", build_root);
        if let Ok(mounts) = remote::run_with_output(host, &mount_check) {
            for mnt in mounts.lines() {
                if !mnt.trim().is_empty() {
                    remote::run(host, &format!("{}umount -f {}", cmd_prefix, mnt.trim())).ok();
                }
            }
        }
        
        // Remove dir
        // Ensure no flags prevent deletion
        remote::run(host, &format!("{}chflags -R noschg {}", cmd_prefix, build_root)).ok();
        remote::run(host, &format!("{}rm -rf {}", cmd_prefix, build_root))?;
    }
    
    // 1. Create Build Jail Structure (Skeleton)
    // Same as jail::create but hardcoded for build
    let base_dir = format!("/usr/local/bsdeploy/base/{}", base_version);
    
    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, build_root))?;

    // Skeleton Copy/Mount
    let rw_dirs = vec!["etc", "var", "root", "tmp"];
    for dir in rw_dirs {
        remote::run(host, &format!("{}cp -a {}/{} {}/", cmd_prefix, base_dir, dir, build_root))?;
    }
    // Resolv.conf
    remote::run(host, &format!("{}cp /etc/resolv.conf {}/etc/", cmd_prefix, build_root))?;
    // Home
    remote::run(host, &format!("{}mkdir -p {}/home", cmd_prefix, build_root))?;

    // Mounts
    let root_mounts = vec!["bin", "lib", "libexec", "sbin"];
    for dir in &root_mounts {
         remote::run(host, &format!("{}mkdir -p {}/{}", cmd_prefix, build_root, dir))?;
         remote::run(host, &format!("{}mount_nullfs -o ro {}/{} {}/{}", cmd_prefix, base_dir, dir, build_root, dir))?;
    }
    // /usr mounts
    remote::run(host, &format!("{}mkdir -p {}/usr", cmd_prefix, build_root))?;
    let usr_mounts = vec!["bin", "include", "lib", "lib32", "libdata", "libexec", "sbin", "share"];
    for dir in &usr_mounts {
         if remote::run(host, &format!("test -d {}/usr/{}", base_dir, dir)).is_ok() {
             remote::run(host, &format!("{}mkdir -p {}/usr/{}", cmd_prefix, build_root, dir))?;
             remote::run(host, &format!("{}mount_nullfs -o ro {}/usr/{} {}/usr/{}", cmd_prefix, base_dir, dir, build_root, dir))?;
         }
    }
    // /usr/local writable
    remote::run(host, &format!("{}mkdir -p {}/usr/local", cmd_prefix, build_root))?;
    
    // Devfs
    remote::run(host, &format!("{}mkdir -p {}/dev", cmd_prefix, build_root))?;
    remote::run(host, &format!("{}mount -t devfs devfs {}/dev", cmd_prefix, build_root))?;

    // 2. Start Jail (Inherit Network)
    let start_cmd = format!(
        "{}jail -c name={} path={} host.hostname={} ip4=inherit allow.raw_sockets=1 persist",
        cmd_prefix, build_jail_name, build_root, build_jail_name
    );
    remote::run(host, &start_cmd)?;

    // 3. Install Packages
    spinner.set_message(format!("[{}] Image: Installing packages...", host));
    remote::run(host, &format!("{}pkg -j {} install -y git bash", cmd_prefix, build_jail_name))?;
    if !config.packages.is_empty() {
        let pkgs = config.packages.join(" ");
        remote::run(host, &format!("{}pkg -j {} install -y {}", cmd_prefix, build_jail_name, pkgs))?;
    }

    // 4. Create User
    if let Some(user) = &config.user {
        // Check if user exists in jail
        let check_user = format!("{}jexec {} id {}", cmd_prefix, build_jail_name, user);
        if remote::run(host, &check_user).is_err() {
            remote::run(host, &format!("{}jexec {} pw useradd -n {} -m -s /usr/local/bin/bash", cmd_prefix, build_jail_name, user))?;
        }
    }

    // 5. Install Mise
    if !config.mise.is_empty() {
        spinner.set_message(format!("[{}] Image: Installing Mise runtimes...", host));
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

    // 5.5 Cleanup Pkg Cache to save space
    remote::run(host, &format!("{}pkg -j {} clean -y", cmd_prefix, build_jail_name)).ok();

    // 6. Stop Jail & Cleanup Mounts
    remote::run(host, &format!("{}jail -r {}", cmd_prefix, build_jail_name))?;
    // Unmount devfs
    remote::run(host, &format!("{}umount {}/dev", cmd_prefix, build_root))?;
    // Unmount RO layers
    // We need to unmount deeply. Reverse order of creation helps, or 'umount -f'
    // Let's be polite.
    for dir in &usr_mounts {
        remote::run(host, &format!("{}umount {}/usr/{}", cmd_prefix, build_root, dir)).ok();
    }
    for dir in &root_mounts {
        remote::run(host, &format!("{}umount {}/{}", cmd_prefix, build_root, dir)).ok();
    }

    // 7. Capture Image
    spinner.set_message(format!("[{}] Image: Saving artifact...", host));
    
    // Create ZFS dataset if parent is ZFS
    if let Ok(Some(images_parent_ds)) = remote::get_zfs_dataset(host, "/usr/local/bsdeploy/images") {
        let image_ds = format!("{}/{}", images_parent_ds, short_hash);
        if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", image_ds)).is_err() {
            // Explicitly set mountpoint to ensure it matches image_path
            remote::run(host, &maybe_doas(&format!("zfs create -o mountpoint={} {}", image_path, image_ds), config.doas))?;
        }
    } else {
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, image_path))?;
    }
    
    // Copy the RW directories using rsync to be more robust
    // We exclude var/empty because it has schg flag and fails cp/rsync
    let save_dirs = vec!["usr/local", "home", "etc", "var", "root"];
    for dir in save_dirs {
        // Ensure source exists
        if remote::run(host, &format!("test -d {}/{}", build_root, dir)).is_err() {
            continue;
        }

        let parent = std::path::Path::new(dir).parent().map(|p| p.to_str().unwrap()).unwrap_or("");
        if !parent.is_empty() {
             remote::run(host, &format!("{}mkdir -p {}/{}", cmd_prefix, image_path, parent))?;
        }
        
        // Use rsync -a source/ destination/ to copy contents correctly
        // We use trailing slash on source to copy contents into the destination dir
        let dest_dir = if parent.is_empty() { image_path.clone() } else { format!("{}/{}", image_path, parent) };
        let rsync_cmd = format!(
            "{}rsync -a --exclude 'var/empty' {}/{} {}/",
            cmd_prefix, build_root, dir, dest_dir
        );
        remote::run(host, &rsync_cmd)?;
    }

    // Manually recreate var/empty
    remote::run(host, &format!("{}mkdir -p {}/var/empty", cmd_prefix, image_path))?;
    remote::run(host, &format!("{}chmod 555 {}/var/empty", cmd_prefix, image_path))?;

    // 7.5 Create ZFS Snapshot if available
    if let Ok(Some(dataset)) = remote::get_zfs_dataset(host, &image_path) {
        spinner.set_message(format!("[{}] Image: Creating ZFS snapshot...", host));
        // Check if snapshot already exists
        let snap_name = format!("{}@base", dataset);
        if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", snap_name)).is_err() {
            remote::run(host, &format!("{}zfs snapshot {}", cmd_prefix, snap_name))?;
        }
    }

    // 8. Destroy Build Jail Root
    spinner.set_message(format!("[{}] Image: Cleaning up build jail...", host));
    remote::run(host, &format!("{}chflags -R noschg {}", cmd_prefix, build_root)).ok();
    remote::run(host, &format!("{}rm -rf {}", cmd_prefix, build_root))?;

    Ok(image_path)
}