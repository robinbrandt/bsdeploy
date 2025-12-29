# bsdeploy Design Document

## 1. Overview
`bsdeploy` is a command-line tool written in Rust designed to deploy applications to FreeBSD servers using FreeBSD Jails for isolation and zero-downtime deployments. It aims for simplicity and convention over configuration, managing system dependencies, environment variables, and application lifecycle through immutable, versioned jail deployments with blue/green traffic switching.

## 2. Configuration
The deployment is defined in `config/bsdeploy.yml` at the root of the application repository.

### Example Configuration
```yaml
service: myapp

hosts:
  - your-server.example.com

# Jail-specific configuration (optional, with defaults)
jail:
  base_version: "14.1-RELEASE"  # Optional, defaults to host version
  ip_range: "10.0.0.0/24"       # Optional, defaults to 10.0.0.0/24

packages:
  - libyaml
  - curl

mise:
  ruby: 3.4.7

env:
  clear:
    - RAILS_ENV: production
    - PORT: "3000"
  secret:
    - SECRET_KEY_BASE

before_start:
  - bin/rails assets:precompile
  - bin/rails db:migrate

start:
  - bin/rails server

data_directories:
  - /var/bsdeploy/myapp/storage: /app/storage

proxy:
  hostname: myapp.example.com
  port: 3000
```

### Data Structures
- **Config**: Root struct matching the YAML.
- **Env**: Struct with `clear` and `secret` maps/lists.

## 3. Architecture

### 3.1 CLI
Uses `clap` for argument parsing.
- Global flags: `--config <path>` (defaulting to `config/bsdeploy.yml`).
- Subcommands: `setup`, `deploy`, `destroy`.

### 3.2 SSH & Remote Execution
The tool needs to execute commands on remote FreeBSD hosts.
- **Strategy**: Use the `ssh2` crate (libssh2 bindings) for programmatic control, or wrap the system `ssh` binary via `std::process::Command`.
- **Decision**: For the initial "simple" version, wrapping the system `ssh` and `scp`/`rsync` binaries is often more robust regarding SSH config (`~/.ssh/config`), agent forwarding, and keys than re-implementing auth logic with `ssh2`. However, for complex output handling, a library is better. We will start by wrapping `ssh` commands for simplicity and native config support.

### 3.3 File Transfer
- Use `rsync` to transfer application code to the remote host.
- Code is synced to versioned jail directories: `/usr/local/bsdeploy/jails/<service>-<timestamp>/app`.

## 4. Subcommands

### 4.1 `setup`
**Goal**: Prepare the host.
1. **Connect**: Verify SSH access.
2. **Bootstrap**: Ensure basic tools are present (e.g., `rsync`, `git` if needed).
3. **Packages**:
    - Update pkg repo: `pkg update`.
    - Install `caddy` (default router).
    - Install user-defined `packages`.
4. **Environment**:
    - Create a `.env` file or strictly configured environment directory on the remote host (e.g., `/usr/local/etc/bsdeploy/<service>/env`).
    - `clear` vars are written directly.
    - `secret` vars are read from the local environment (or a vault) and written to the remote.

### 4.2 `deploy`
**Goal**: Ship and run code with zero downtime using FreeBSD jails.
1. **Prepare Jail**:
    - Download and cache FreeBSD base system if not already present
    - Create versioned jail: `<service>-<timestamp>`
    - Mount base system read-only via nullfs
    - Assign unique IP address from configured range (default: 10.0.0.0/24)
2. **Build Image** (cached for performance):
    - Create content-addressed image based on packages, mise tools, and base version
    - Install packages inside jail using `pkg -j`
    - Install mise tools inside jail
3. **Ship Code**:
    - `rsync` current directory to jail app directory
    - Exclude `.git`, data directories, etc.
4. **Prepare Application**:
    - Create configured user inside jail
    - Mount data directories from host to jail
    - Run `before_start` commands inside jail
5. **Start Application**:
    - Run `start` commands inside jail
    - Processes run daemonized with logging
6. **Blue/Green Cutover**:
    - Update Caddy reverse proxy configuration to point to new jail's IP
    - Reload Caddy for zero-downtime traffic switch
7. **Cleanup**:
    - Stop old jails
    - Remove old jails (keeps last 3 for rollback)

### 4.3 `destroy`
**Goal**: Remove all resources for a service.
1. Stop and remove all jails for the service
2. Remove IP aliases
3. Remove Caddy configuration
4. Reload Caddy

## 5. Deployment Architecture

### 5.1 Jail-Based Deployment
All deployments use FreeBSD jails for:
- **Process Isolation**: Each deployment runs in its own jail
- **Zero-Downtime Updates**: Blue/green deployment with traffic switching
- **Instant Rollbacks**: Keep previous jail versions for quick rollback
- **Clean State**: Destroying a jail removes all traces

### 5.2 Networking
- **Loopback IP Aliasing**: Jails use IP aliases on `lo1` interface
- **Reverse Proxy**: Caddy on host proxies traffic to jail IPs
- **Private Network**: Jails communicate via internal IPs (default: 10.0.0.0/24)

### 5.3 Image Caching
- Content-addressed images based on SHA256 hash of: packages + mise tools + base version
- Shared across deployments with identical dependencies
- ZFS cloning used when available for fast, space-efficient copies

## 6. Breaking Changes (v1.0.0)

**Note**: Version 1.0.0 removes support for host-based deployments. All deployments now use FreeBSD jails.

- Removed `strategy` field from configuration (jail is now the only deployment method)
- Removed `restart` command (use `deploy` to create a new jail instead)
- If upgrading from a host deployment, you'll need to:
  1. Remove the `strategy` field from your configuration file
  2. Run `destroy` to clean up old host deployment (if needed, manually clean `/var/db/bsdeploy`, `/usr/local/etc/bsdeploy`, etc.)
  3. Run `setup` and `deploy` to create jail-based deployment
