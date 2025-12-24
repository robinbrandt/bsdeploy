use crate::remote;
use anyhow::{Context, Result, anyhow};
use chrono::Local;

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
    let used_ips: Vec<String> = output.lines().map(|s| s.trim().to_string()).collect();

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
    let base_dir = format!("/usr/local/bsdeploy/base/{}", version);
    let cmd_prefix = if doas { "doas " } else { "" };
    
    // Check if base exists (checking /bin inside)
    let check_cmd = format!("test -d {}/bin", base_dir);
    if remote::run(host, &format!("{}{}", cmd_prefix, check_cmd)).is_ok() {
        return Ok(());
    }

    // Create directory
    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, base_dir))?;

    // Fetch and extract
    // We assume 14.1-RELEASE format.
    // Defaulting to amd64.
    let url = format!("https://download.freebsd.org/ftp/releases/amd64/{}/base.txz", version);
    
    // We stream fetch into tar to save disk space and time
    // Note: This might take a while, so we rely on the caller to show a spinner/message.
    let fetch_cmd = format!(
        "{}fetch -o - {} | {}tar -xf - -C {}", 
        cmd_prefix, url, cmd_prefix, base_dir
    );

    remote::run(host, &fetch_cmd).with_context(|| format!("Failed to fetch and extract base system version {}", version))?;

    Ok(())
}

pub struct JailInfo {
    pub name: String,
    pub path: String,
    pub ip: String,
}

pub fn create(host: &str, service: &str, base_version: &str, subnet: &str, image_path: Option<&str>, data_dirs: &[String], doas: bool) -> Result<JailInfo> {
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
    
    if let Some(img) = image_path {
        // Copy RW dirs from Image (excluding usr/local)
        let rw_dirs = vec!["etc", "var", "root", "home"];
        for dir in rw_dirs {
            remote::run(host, &format!("{}cp -a {}/{} {}/", cmd_prefix, img, dir, jail_root))?;
        }
        
        // MOUNT /usr/local from Image (Read-Only)
        remote::run(host, &format!("{}mkdir -p {}/usr/local", cmd_prefix, jail_root))?;
        remote::run(host, &format!("{}mount_nullfs -o ro {}/usr/local {}/usr/local", cmd_prefix, img, jail_root))?;
        
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
    
    // Devfs
    remote::run(host, &format!("{}mkdir -p {}/dev", cmd_prefix, jail_root))?;
    remote::run(host, &format!("{}mount -t devfs devfs {}/dev", cmd_prefix, jail_root))?;

    // Data Directories (Host -> Jail nullfs RW)
    for dir in data_dirs {
        // Ensure host dir exists
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, dir))?;
        // Ensure jail mountpoint exists
        let jail_mount_path = format!("{}{}", jail_root, dir);
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_mount_path))?;
        // Mount
        remote::run(host, &format!("{}mount_nullfs {} {}", cmd_prefix, dir, jail_mount_path))?;
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
