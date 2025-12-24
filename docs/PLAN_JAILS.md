# Plan: FreeBSD Jails Support (Blue/Green Deployment)

## Objective
Transition `bsdeploy` from "bare metal" mutable deployments to **immutable, versioned deployments** using FreeBSD Jails. This enables:
- **Zero-downtime updates** (Blue/Green).
- **Instant rollbacks**.
- **Process isolation** (security and dependency conflicts).
- **Clean system state** (destroying a jail removes all traces of that version).

## Architecture

### The "Thin Jail" Model
To ensure speed and efficiency, we will use a "Thin Jail" approach similar to tools like Bastille or iocage, but managed directly by `bsdeploy` to avoid external dependencies.

1.  **Base System**: fetched once to `/usr/local/bsdeploy/base/14.1-RELEASE` (or matching host).
2.  **Jail Root**: `/usr/local/bsdeploy/jails/<service>-<timestamp>`.
3.  **Mounts**:
    - The Base System is mounted **Read-Only** via `nullfs` into the Jail Root.
    - `/dev` is mounted.
    - **App Directory**: The code is copied into a private writable directory (e.g., `/app`).
    - **Data Directories**: Persistent data (configured in `bsdeploy.yml`) is mounted `nullfs` (Read-Write) from the Host to the Jail.

### Networking (Loopback Proxy)
We will use **IP Aliasing on the Loopback Interface (`lo1`)**.
- **Host**: Runs Caddy.
- **Jail**: Assigned a unique internal IP (e.g., `10.0.0.5`).
- **Traffic**: Caddy (Host) -> Reverse Proxy -> Jail IP:Port.

This avoids the complexity of VNET/Bridge setup for now, while perfectly satisfying the HTTP deployment use case.

## Configuration Updates (`bsdeploy.yml`)

```yaml
# New 'strategy' field
strategy: jail # vs 'host' (default)

# Jail specific settings (optional, defaults provided)
jail:
  base_version: "14.2-RELEASE" # Defaults to host version
  interface: "lo1"             # Interface to alias IPs on
  ip_range: "10.0.0.0/24"      # subnet for auto-assignment

# Existing fields behave differently:
# 'packages': Installed INSIDE the jail via pkg -j
# 'user': Created INSIDE the jail
# 'data_directories': Mounted from Host -> Jail
```

## Deployment Lifecycle (Blue/Green)

1.  **Prepare Host**:
    - Ensure `lo1` interface exists.
    - Ensure Base System is fetched (if not, `fetch` and extract).
2.  **Provision Jail (Green)**:
    - Generate ID: `myapp-20241224-1200`.
    - Find free IP in range (e.g., `10.0.0.3`).
    - Create directory structure.
    - Mount `base`, `devfs`, and `data_directories`.
    - Create `jail.conf` entry (ephemeral or persistent file).
    - Start Jail.
3.  **Setup Environment**:
    - `pkg -j <jail> install <packages>`
    - Create user inside Jail.
    - Install `mise` and tools inside Jail.
    - Copy application code to Jail.
4.  **Start Application**:
    - Run `before_start` hooks inside Jail.
    - Start service (daemonized) inside Jail.
    - **Health Check**: Curl the Jail IP to ensure it's up.
5.  **Switch Traffic (The "Cutover")**:
    - Update Caddy config on Host to point to `10.0.0.3`.
    - `service caddy reload`.
6.  **Cleanup (Blue)**:
    - Identify previous deployment Jail.
    - Stop and remove the old Jail (unmount, delete dir).

## Implementation Phases

### Phase 1: Jail Primitives
- Implement `jail::setup_base()`: Fetch/extract FreeBSD base.
- Implement `jail::create()`: Directory setup, fstab generation, `jail -c`.
- Implement `jail::exec()`: Running commands inside (replacing current SSH `run` logic).

### Phase 2: Networking & IP Management
- Logic to find an unused IP in the subnet on the remote host.
- Managing `ifconfig lo1 alias`.

### Phase 3: The Deployment Logic
- Stitching it together: New `deploy_jail` function in `main.rs`.
- Handling data persistence (bind mounts).

### Phase 4: Cleanup & Pruning
- Logic to detect old jails and remove them.
- "Rollback" command (point Caddy back to previous IP).
