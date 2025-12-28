use std::process::{Command, Stdio};
use anyhow::{Context, Result, anyhow};
use log::debug;
use std::io::Write;

use crate::shell;

pub fn run(host: &str, command: &str) -> Result<()> {
    debug!("SSH [{}] Executing: {}", host, command);
    let output = Command::new("ssh")
        .arg(host)
        .arg(command)
        .output() // Captures stdout/stderr
        .with_context(|| format!("Failed to execute ssh command on {}", host))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!("Stdout: {}", stdout);
        debug!("Stderr: {}", stderr);
        return Err(anyhow!("Command failed on {}: {}. Error: {}", host, command, stderr.trim()));
    }
    Ok(())
}

#[allow(dead_code)]
pub fn run_with_output(host: &str, command: &str) -> Result<String> {
    debug!("SSH [{}] Executing (output): {}", host, command);
    let output = Command::new("ssh")
        .arg(host)
        .arg(command)
        .output()
        .with_context(|| format!("Failed to execute ssh command on {}", host))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Command failed on {}: {}. Error: {}", host, command, stderr));
    }
    
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn get_os_release(host: &str) -> Result<String> {
    let output = run_with_output(host, "uname -r")?;
    Ok(output.trim().to_string())
}

pub fn write_file(host: &str, content: &str, dest_path: &str, use_doas: bool) -> Result<()> {
    debug!("SSH [{}] Writing file: {}", host, dest_path);

    let safe_path = shell::escape(dest_path);
    let remote_cmd = if use_doas {
        format!("doas tee {} > /dev/null", safe_path)
    } else {
        format!("cat > {}", safe_path)
    };

    let mut child = Command::new("ssh")
        .arg(host)
        .arg(remote_cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null()) // Suppress stdout
        .stderr(Stdio::piped()) // Capture stderr
        .spawn()
        .with_context(|| format!("Failed to spawn ssh for file writing on {}", host))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(content.as_bytes())
            .with_context(|| "Failed to write content to ssh stdin")?;
    }

    let output = child.wait_with_output().with_context(|| "Failed to wait for ssh process")?;
    
    if !output.status.success() {
         let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to write file {} on {}: {}", dest_path, host, stderr.trim()));
    }
    Ok(())
}

pub fn sync(host: &str, src: &str, dest: &str, excludes: &[String], use_doas: bool) -> Result<()> {
    debug!("Syncing {} to {}:{}", src, host, dest);
    // Ensure rsync is installed locally
    let mut cmd = Command::new("rsync");
    cmd.arg("-az")
       .arg("--delete-delay")      // Delete after transfer, not during (safer)
       .arg("--timeout=30")         // Prevent hanging on network issues
       .arg("--filter=:- .gitignore")
       .arg("--exclude=.git")
       .arg("--exclude=node_modules")
       .arg("--exclude=tmp")
       .arg("--exclude=log");
    
    for ex in excludes {
        cmd.arg(format!("--exclude={}", ex));
    }
    
    if use_doas {
        cmd.arg("--rsync-path=doas rsync");
    }

    let output = cmd
        .arg(src)
        .arg(format!("{}:{}", host, dest))
        .output() // Capture output
        .with_context(|| "Failed to execute rsync")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Failed to sync files to {}: {}", host, stderr.trim()));
    }
    Ok(())
}

/// Detect if a path is on a ZFS dataset and return the dataset name
pub fn get_zfs_dataset(host: &str, path: &str) -> Result<Option<String>> {
    // 1. Find the mountpoint for the path using df
    // df -p is POSIX but might not give exactly what we want.
    // On FreeBSD, 'df <path>' shows the mountpoint in the first column if it's a device/dataset.
    let safe_path = shell::escape(path);
    let df_cmd = format!("df {} | tail -n 1 | awk '{{print $1}}'", safe_path);
    let dataset_candidate = match run_with_output(host, &df_cmd) {
        Ok(out) => out.trim().to_string(),
        Err(_) => return Ok(None),
    };

    if dataset_candidate.is_empty() || dataset_candidate.starts_with('/') {
        // Not a ZFS dataset (likely a regular path or something else)
        return Ok(None);
    }

    // 2. Verify it's a ZFS dataset
    let zfs_cmd = format!("zfs list -H -o name {} 2>/dev/null", dataset_candidate);
    let output = match run_with_output(host, &zfs_cmd) {
        Ok(out) => out,
        Err(_) => {
            let doas_cmd = format!("doas zfs list -H -o name {} 2>/dev/null", dataset_candidate);
            match run_with_output(host, &doas_cmd) {
                Ok(out) => out,
                Err(_) => return Ok(None),
            }
        }
    };

    let name = output.trim().to_string();
    if name.is_empty() {
        Ok(None)
    } else {
        debug!("Detected ZFS dataset {} for path {}", name, path);
        Ok(Some(name))
    }
}