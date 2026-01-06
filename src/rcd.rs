use anyhow::Result;

use crate::constants::ACTIVE_DIR;
use crate::remote;

/// RC.D script for bsdeploy boot persistence
const RCD_SCRIPT: &str = r#"#!/bin/sh

# PROVIDE: bsdeploy
# REQUIRE: NETWORKING
# BEFORE: caddy
# KEYWORD: shutdown

. /etc/rc.subr

name="bsdeploy"
rcvar="bsdeploy_enable"
start_cmd="${name}_start"
stop_cmd="${name}_stop"
status_cmd="${name}_status"
restart_cmd="${name}_restart"
extra_commands="status"

ACTIVE_DIR="/usr/local/bsdeploy/active"
JAILS_DIR="/usr/local/bsdeploy/jails"
BASE_DIR="/usr/local/bsdeploy/base"
JQ="/usr/local/bin/jq"

bsdeploy_start()
{
    echo "Starting bsdeploy jails..."

    # Ensure lo1 exists
    if ! ifconfig lo1 > /dev/null 2>&1; then
        ifconfig lo1 create
    fi

    # Iterate over active services
    for link in "$ACTIVE_DIR"/*; do
        [ -L "$link" ] || continue

        jail_path=$(readlink -f "$link")
        [ -d "$jail_path" ] || continue

        metadata="$jail_path/.bsdeploy.json"
        [ -f "$metadata" ] || continue

        # Parse metadata using jq
        jail_name=$($JQ -r '.jail_name' "$metadata")
        ip=$($JQ -r '.ip' "$metadata")
        service=$($JQ -r '.service' "$metadata")
        user=$($JQ -r '.user // empty' "$metadata")
        base_version=$($JQ -r '.base_version' "$metadata")
        image_path=$($JQ -r '.image_path // empty' "$metadata")
        is_zfs=$($JQ -r '.zfs' "$metadata")

        echo "  Starting $service ($jail_name)..."

        # 1. Add IP alias to lo1
        if [ -n "$ip" ]; then
            ifconfig lo1 inet "$ip/32" alias 2>/dev/null
        fi

        # 2. Mount filesystems based on ZFS or non-ZFS
        bsdeploy_mount_jail "$jail_path" "$base_version" "$image_path" "$is_zfs" "$metadata"

        # 3. Start jail
        jail -c name="$jail_name" path="$jail_path" host.hostname="$jail_name" \
            ip4.addr="$ip" allow.raw_sockets=1 persist

        # 4. Start application processes
        bsdeploy_start_processes "$metadata" "$jail_name" "$service" "$user"
    done
}

bsdeploy_mount_jail()
{
    local jail_path="$1"
    local base_version="$2"
    local image_path="$3"
    local is_zfs="$4"
    local metadata="$5"
    local base_dir="$BASE_DIR/$base_version"

    # Always mount devfs
    mkdir -p "$jail_path/dev" 2>/dev/null
    mount -t devfs devfs "$jail_path/dev" 2>/dev/null

    if [ "$is_zfs" = "true" ]; then
        # ZFS clone - base system is already in the clone, only mount data directories
        :
    else
        # Non-ZFS: mount base system and image via nullfs
        for dir in bin lib libexec sbin; do
            [ -d "$base_dir/$dir" ] && mount_nullfs -o ro "$base_dir/$dir" "$jail_path/$dir" 2>/dev/null
        done

        for dir in bin include lib lib32 libdata libexec sbin share; do
            [ -d "$base_dir/usr/$dir" ] && mount_nullfs -o ro "$base_dir/usr/$dir" "$jail_path/usr/$dir" 2>/dev/null
        done

        # Mount image /usr/local if specified
        if [ -n "$image_path" ] && [ -d "$image_path/usr/local" ]; then
            mount_nullfs -o ro "$image_path/usr/local" "$jail_path/usr/local" 2>/dev/null
        fi
    fi

    # Mount data directories
    $JQ -r '.data_directories[]? | "\(.host_path) \(.jail_path)"' "$metadata" 2>/dev/null | while read host_path jail_path_rel; do
        if [ -n "$host_path" ] && [ -n "$jail_path_rel" ]; then
            jail_path_rel=$(echo "$jail_path_rel" | sed 's|^/||')
            target="${jail_path}/${jail_path_rel}"
            mkdir -p "$target" 2>/dev/null
            mount_nullfs "$host_path" "$target" 2>/dev/null
        fi
    done
}

bsdeploy_start_processes()
{
    local metadata="$1"
    local jail_name="$2"
    local service="$3"
    local user="$4"

    local env_file="/etc/bsdeploy.env"
    local app_dir="/app"
    local run_dir="/var/run/bsdeploy/$service"
    local log_dir="/var/log/bsdeploy/$service"

    local idx=0
    $JQ -r '.start_commands[]' "$metadata" 2>/dev/null | while read start_cmd; do
        [ -z "$start_cmd" ] && continue

        local pid_file="$run_dir/service.pid"
        local log_file="$log_dir/service.log"

        # Build daemon command
        local daemon_cmd="daemon -f -p $pid_file -o $log_file"
        if [ -n "$user" ]; then
            daemon_cmd="$daemon_cmd -u $user"
        fi

        local full_cmd="$daemon_cmd bash -c 'source $env_file && cd $app_dir && $start_cmd'"
        jexec "$jail_name" sh -c "$full_cmd"

        idx=$((idx + 1))
    done
}

bsdeploy_stop()
{
    echo "Stopping bsdeploy jails..."

    for link in "$ACTIVE_DIR"/*; do
        [ -L "$link" ] || continue

        jail_path=$(readlink -f "$link")
        [ -d "$jail_path" ] || continue

        metadata="$jail_path/.bsdeploy.json"
        [ -f "$metadata" ] || continue

        jail_name=$($JQ -r '.jail_name' "$metadata")
        ip=$($JQ -r '.ip' "$metadata")
        service=$($JQ -r '.service' "$metadata")

        echo "  Stopping $service ($jail_name)..."

        # Stop jail (this also stops all processes inside)
        jail -r "$jail_name" 2>/dev/null

        # Remove IP alias
        if [ -n "$ip" ]; then
            ifconfig lo1 inet "$ip" -alias 2>/dev/null
        fi

        # Unmount filesystems
        for mnt in $(mount | grep "$jail_path" | awk '{print $3}' | sort -r); do
            umount -f "$mnt" 2>/dev/null
        done
    done
}

bsdeploy_status()
{
    echo "bsdeploy jail status:"

    if [ ! -d "$ACTIVE_DIR" ] || [ -z "$(ls -A "$ACTIVE_DIR" 2>/dev/null)" ]; then
        echo "  No active services"
        return
    fi

    for link in "$ACTIVE_DIR"/*; do
        [ -L "$link" ] || continue

        service=$(basename "$link")
        jail_path=$(readlink -f "$link")

        if [ ! -d "$jail_path" ]; then
            echo "  $service: BROKEN (symlink points to non-existent path)"
            continue
        fi

        metadata="$jail_path/.bsdeploy.json"
        if [ ! -f "$metadata" ]; then
            echo "  $service: BROKEN (missing metadata)"
            continue
        fi

        jail_name=$($JQ -r '.jail_name' "$metadata")

        if jls -j "$jail_name" > /dev/null 2>&1; then
            ip=$(jls -j "$jail_name" ip4.addr 2>/dev/null)
            echo "  $service: RUNNING ($jail_name, IP: $ip)"
        else
            echo "  $service: STOPPED ($jail_name)"
        fi
    done
}

bsdeploy_restart()
{
    bsdeploy_stop
    bsdeploy_start
}

load_rc_config $name
run_rc_command "$1"
"#;

/// Install the rc.d script on the remote host
pub fn install_rcd_script(host: &str, doas: bool) -> Result<()> {
    let rcd_path = "/usr/local/etc/rc.d/bsdeploy";

    // Write the rc.d script
    remote::write_file(host, RCD_SCRIPT, rcd_path, doas)?;

    // Make it executable
    let cmd_prefix = if doas { "doas " } else { "" };
    remote::run(host, &format!("{}chmod +x {}", cmd_prefix, rcd_path))?;

    Ok(())
}

/// Enable the bsdeploy service to start on boot
pub fn enable_service(host: &str, doas: bool) -> Result<()> {
    let cmd_prefix = if doas { "doas " } else { "" };
    remote::run(host, &format!("{}sysrc bsdeploy_enable=YES", cmd_prefix))?;
    Ok(())
}

/// Create the active directory for symlinks
pub fn ensure_active_dir(host: &str, doas: bool) -> Result<()> {
    let cmd_prefix = if doas { "doas " } else { "" };
    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, ACTIVE_DIR))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rcd_script_has_required_sections() {
        // Test that the rc.d script has all required FreeBSD rc.d components
        assert!(RCD_SCRIPT.contains("# PROVIDE: bsdeploy"));
        assert!(RCD_SCRIPT.contains("# REQUIRE: NETWORKING"));
        assert!(RCD_SCRIPT.contains("# BEFORE: caddy"));
        assert!(RCD_SCRIPT.contains(". /etc/rc.subr"));
        assert!(RCD_SCRIPT.contains("load_rc_config $name"));
        assert!(RCD_SCRIPT.contains("run_rc_command"));
    }

    #[test]
    fn test_rcd_script_has_start_stop_status() {
        // Test that start, stop, and status commands are defined
        assert!(RCD_SCRIPT.contains("bsdeploy_start()"));
        assert!(RCD_SCRIPT.contains("bsdeploy_stop()"));
        assert!(RCD_SCRIPT.contains("bsdeploy_status()"));
        assert!(RCD_SCRIPT.contains("bsdeploy_restart()"));
    }

    #[test]
    fn test_rcd_script_uses_correct_paths() {
        // Test that the script uses the correct bsdeploy paths
        assert!(RCD_SCRIPT.contains(r#"ACTIVE_DIR="/usr/local/bsdeploy/active""#));
        assert!(RCD_SCRIPT.contains(r#"JAILS_DIR="/usr/local/bsdeploy/jails""#));
        assert!(RCD_SCRIPT.contains(r#"BASE_DIR="/usr/local/bsdeploy/base""#));
    }

    #[test]
    fn test_rcd_script_handles_zfs_and_non_zfs() {
        // Test that the script distinguishes between ZFS and non-ZFS jails
        assert!(RCD_SCRIPT.contains(r#"is_zfs=$($JQ -r '.zfs' "$metadata")"#));
        assert!(RCD_SCRIPT.contains(r#"if [ "$is_zfs" = "true" ]"#));
    }

    #[test]
    fn test_rcd_script_uses_jq_for_json() {
        // Test that the script uses jq to parse JSON metadata
        assert!(RCD_SCRIPT.contains("$JQ -r '.jail_name'"));
        assert!(RCD_SCRIPT.contains("$JQ -r '.ip'"));
        assert!(RCD_SCRIPT.contains("$JQ -r '.service'"));
        assert!(RCD_SCRIPT.contains("$JQ -r '.start_commands[]'"));
    }

    #[test]
    fn test_rcd_script_creates_lo1() {
        // Test that the script creates lo1 interface if needed
        assert!(RCD_SCRIPT.contains("ifconfig lo1 create"));
    }

    #[test]
    fn test_rcd_script_mounts_devfs() {
        // Test that the script mounts devfs
        assert!(RCD_SCRIPT.contains("mount -t devfs devfs"));
    }

    #[test]
    fn test_rcd_script_starts_jail_correctly() {
        // Test that the jail start command has correct parameters
        assert!(RCD_SCRIPT.contains("jail -c name="));
        assert!(RCD_SCRIPT.contains("allow.raw_sockets=1"));
        assert!(RCD_SCRIPT.contains("persist"));
    }

    #[test]
    fn test_rcd_script_stops_jail_correctly() {
        // Test that the script stops jails properly
        assert!(RCD_SCRIPT.contains("jail -r"));
    }

    #[test]
    fn test_rcd_script_handles_ip_aliases() {
        // Test that the script manages IP aliases on lo1
        assert!(RCD_SCRIPT.contains("ifconfig lo1 inet"));
        assert!(RCD_SCRIPT.contains("-alias"));
    }
}
