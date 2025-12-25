use crate::remote;
use anyhow::{Context, Result, anyhow};
use chrono::Local;
use std::collections::HashSet;

fn find_free_ip(host: &str, subnet: &str, _doas: bool) -> Result<String> {
    // Default 10.0.0.0/24
    // We scan 10.0.0.2 to 10.0.0.254
    // subnet format: "10.0.0.0/24"
    
    // Parse base
    let base_ip = subnet.split('/').next().unwrap_or("10.0.0.0");
    let parts: Vec<&str> = base_ip.split('.').collect();
    if parts.len() != 4 {
        return Err(anyhow!("Invalid subnet format"));
    }
    let prefix = format!("{}.{}.{}", parts[0], parts[1], parts[2]);

    // Get current aliases on lo1
    let cmd = "ifconfig lo1 | grep 'inet ' | awk '{print $2}'";
    let output = remote::run_with_output(host, cmd)?;
    // Use HashSet for O(1) lookup instead of O(n) Vec::contains
    let used_ips: HashSet<String> = output.lines().map(|s| s.trim().to_string()).collect();

    for i in 2..255 {
        let candidate = format!("{}.{}", prefix, i);
        if !used_ips.contains(&candidate) {
            // Check if pingable (double check)
            // if !remote::run(host, &format!("ping -c 1 -t 1 {}", candidate)).is_ok() {
                 return Ok(candidate);
            // }
        }
    }

    Err(anyhow!("No free IPs found in subnet {}", subnet))
}

pub fn ensure_base(host: &str, version: &str, doas: bool) -> Result<()> {
    let base_parent_dir = "/usr/local/bsdeploy/base";
    let base_dir = format!("{}/{}", base_parent_dir, version);
    let cmd_prefix = if doas { "doas " } else { "" };
    
    // Check if base is fully ready (marker or just existence)
    // We check for @clean snapshot if ZFS, or just directory if not.
    let is_zfs = remote::get_zfs_dataset(host, base_parent_dir).ok().flatten().is_some();
    
    if is_zfs {
        if let Ok(Some(ds)) = remote::get_zfs_dataset(host, &base_dir) {
            // Dataset exists, check for snapshot
            if remote::run(host, &format!("zfs list -H -o name {}@clean 2>/dev/null", ds)).is_ok() {
                return Ok(());
            }
        }
    } else {
        // Legacy check
        if remote::run(host, &format!("test -d {}/bin", base_dir)).is_ok() {
            return Ok(());
        }
    }

    // Create directory or dataset
    if is_zfs {
         if let Ok(Some(parent_ds)) = remote::get_zfs_dataset(host, base_parent_dir) {
             let target_ds = format!("{}/{}", parent_ds, version);
             // Create dataset if not exists
             if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", target_ds)).is_err() {
                 remote::run(host, &format!("{}zfs create -o mountpoint={} {}", cmd_prefix, base_dir, target_ds))?;
             }
         }
    } else {
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, base_dir))?;
    }

    // Fetch and extract if empty (checking /bin)
    if remote::run(host, &format!("test -d {}/bin", base_dir)).is_err() {
        // We assume 14.1-RELEASE format.
        // Defaulting to amd64.
        let url = format!("https://download.freebsd.org/ftp/releases/amd64/{}/base.txz", version);
        
        let fetch_cmd = format!(
            "{}fetch -o - {} | {}tar -xf - -C {}", 
            cmd_prefix, url, cmd_prefix, base_dir
        );

        remote::run(host, &fetch_cmd).with_context(|| format!("Failed to fetch and extract base system version {}", version))?;
        
        // Copy timezone and resolv.conf for template completeness (though we copy resolv.conf later too)
        remote::run(host, &format!("{}cp /etc/localtime {}/etc/localtime", cmd_prefix, base_dir)).ok();
    }

    // Create ZFS Snapshot if applicable
    if is_zfs {
        if let Ok(Some(ds)) = remote::get_zfs_dataset(host, &base_dir) {
             if remote::run(host, &format!("zfs list -H -o name {}@clean 2>/dev/null", ds)).is_err() {
                 remote::run(host, &format!("{}zfs snapshot {}@clean", cmd_prefix, ds))?;
             }
        }
    }

    Ok(())
}

pub struct JailInfo {
    pub name: String,
    pub path: String,
    pub ip: String,
}

pub fn create(host: &str, service: &str, base_version: &str, subnet: &str, image_path: Option<&str>, data_dirs: &[crate::config::DataDirectory], doas: bool) -> Result<JailInfo> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let jail_name = format!("{}-{}", service, timestamp);
    let jail_root = format!("/usr/local/bsdeploy/jails/{}", jail_name);
    let base_dir = format!("/usr/local/bsdeploy/base/{}", base_version);
    let cmd_prefix = if doas { "doas " } else { "" };

    // 0. Ensure lo1 exists
    // We check if lo1 exists, if not create it
    if remote::run(host, "ifconfig lo1 >/dev/null 2>&1").is_err() {
        remote::run(host, &format!("{}ifconfig lo1 create", cmd_prefix))?;
    }

    // 1. Create Jail Root
    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_root))?;

    // 2. Setup correct structure (Skeleton)
    
    // RW Directories:
    // If image_path is present, copy from image.
    // If not, copy from base (and create empty /usr/local, /home)
    
    let mut zfs_cloned = false;
    if let Some(img) = image_path {
        // Try ZFS clone first
        if let Ok(Some(img_dataset)) = remote::get_zfs_dataset(host, img) {
            let snap_name = format!("{}@base", img_dataset);
            // Check if snapshot exists
            if remote::run(host, &format!("zfs list -H -o name {} 2>/dev/null", snap_name)).is_ok() {
                // Find parent dataset for jails
                if let Ok(Some(jails_parent_dataset)) = remote::get_zfs_dataset(host, "/usr/local/bsdeploy/jails") {
                    let target_dataset = format!("{}/{}", jails_parent_dataset, jail_name);
                    // Clone it and set explicit mountpoint
                    if remote::run(host, &format!("{}zfs clone -o mountpoint={} {} {}", cmd_prefix, jail_root, snap_name, target_dataset)).is_ok() {
                        zfs_cloned = true;
                    }
                }
            }
        }

        if !zfs_cloned {
            // Fallback to Copy RW dirs from Image (excluding usr/local)
            // Use hardlinks to save disk space - identical files shared until modified
            remote::run(host, &format!("{}mkdir -p {}/usr", cmd_prefix, jail_root))?;

            let rw_dirs = vec!["etc", "var", "root", "home"];
            for dir in rw_dirs {
                // Use cp -al for hardlinked copy (UFS-optimized)
                remote::run(host, &format!("{}cp -al {}/{} {}/", cmd_prefix, img, dir, jail_root))?;
            }
        }
        
        // MOUNT /usr/local from Image (Read-Only)
        // (Even with ZFS clone, we might want to mount /usr/local RO if it was part of the image dataset)
        // Actually, if we ZFS cloned the whole image, /usr/local is already there but it's RW.
        // The plan says we mount /usr/local RO from image. 
        // If we ZFS cloned, we might have /usr/local in the clone already.
        // Let's stick to the plan: images store /usr/local. 
        // If we ZFS cloned, we have a full copy of the image.
        
        if zfs_cloned {
            // If we cloned, /usr/local is already there and writable.
            // We do NOT need to mount it.
        } else {
            remote::run(host, &format!("{}mkdir -p {}/usr/local", cmd_prefix, jail_root))?;
            remote::run(host, &format!("{}mount_nullfs -o ro {}/usr/local {}/usr/local", cmd_prefix, img, jail_root))?;
        }
        
    } else {
        // Legacy/Empty Init
        let rw_dirs = vec!["etc", "var", "root", "tmp"];
        for dir in rw_dirs {
            remote::run(host, &format!("{}cp -a {}/{} {}/", cmd_prefix, base_dir, dir, jail_root))?;
        }
        remote::run(host, &format!("{}cp /etc/resolv.conf {}/etc/", cmd_prefix, jail_root))?;
        remote::run(host, &format!("{}mkdir -p {}/home", cmd_prefix, jail_root))?;
        remote::run(host, &format!("{}mkdir -p {}/usr", cmd_prefix, jail_root))?;
        remote::run(host, &format!("{}mkdir -p {}/usr/local", cmd_prefix, jail_root))?;
    }

    // Dirs to create for mounting
    if !zfs_cloned {
        let root_mounts = vec!["bin", "lib", "libexec", "sbin"];
        for dir in root_mounts {
             remote::run(host, &format!("{}mkdir -p {}/{}", cmd_prefix, jail_root, dir))?;
             remote::run(host, &format!("{}mount_nullfs -o ro {}/{} {}/{}", cmd_prefix, base_dir, dir, jail_root, dir))?;
        }

        // Handle /usr mounts (skipping local)
        let usr_mounts = vec!["bin", "include", "lib", "lib32", "libdata", "libexec", "sbin", "share"];
        for dir in usr_mounts {
             if remote::run(host, &format!("test -d {}/usr/{}", base_dir, dir)).is_ok() {
                 remote::run(host, &format!("{}mkdir -p {}/usr/{}", cmd_prefix, jail_root, dir))?;
                 remote::run(host, &format!("{}mount_nullfs -o ro {}/usr/{} {}/usr/{}", cmd_prefix, base_dir, dir, jail_root, dir))?;
             }
        }
    }
    
    // Devfs
    remote::run(host, &format!("{}mkdir -p {}/dev", cmd_prefix, jail_root))?;
    remote::run(host, &format!("{}mount -t devfs devfs {}/dev", cmd_prefix, jail_root))?;

    // Fix permissions for tmp
    remote::run(host, &format!("{}mkdir -p {}/tmp", cmd_prefix, jail_root))?;
    remote::run(host, &format!("{}chmod 1777 {}/tmp", cmd_prefix, jail_root))?;
    remote::run(host, &format!("{}mkdir -p {}/var/tmp", cmd_prefix, jail_root))?;
    remote::run(host, &format!("{}chmod 1777 {}/var/tmp", cmd_prefix, jail_root))?;

    // Data Directories (Host -> Jail nullfs RW)
    for entry in data_dirs {
        let (host_path, jail_path) = entry.get_paths();
        if host_path.is_empty() || jail_path.is_empty() { continue; }

        // Ensure host dir exists
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, host_path))?;
        // Ensure jail mountpoint exists (absolute path relative to jail root)
        // Strip leading slash from jail_path if it exists to join with jail_root
        let target_in_jail = format!("{}/{}", jail_root, jail_path.trim_start_matches('/'));
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, target_in_jail))?;
        // Mount
        remote::run(host, &format!("{}mount_nullfs {} {}", cmd_prefix, host_path, target_in_jail))?;
    }

    // 3. Network Setup
    let ip = find_free_ip(host, subnet, doas)?;
    // Alias the IP on lo1
    remote::run(host, &format!("{}ifconfig lo1 inet {}/32 alias", cmd_prefix, ip))?;

    Ok(JailInfo {
        name: jail_name,
        path: jail_root,
        ip,
    })
}
