use std::process::{Command, Stdio};
use anyhow::{Context, Result, anyhow};
use log::debug;
use std::io::Write;

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
    
    let remote_cmd = if use_doas {
        format!("doas tee {} > /dev/null", dest_path)
    } else {
        format!("cat > {}", dest_path)
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

pub fn sync(host: &str, src: &str, dest: &str, use_doas: bool) -> Result<()> {
    debug!("Syncing {} to {}:{}", src, host, dest);
    // Ensure rsync is installed locally
    let mut cmd = Command::new("rsync");
    cmd.arg("-az")
       .arg("--delete")
       .arg("--filter=:- .gitignore")
       .arg("--exclude=.git")
       .arg("--exclude=node_modules")
       .arg("--exclude=tmp")
       .arg("--exclude=log");
    
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