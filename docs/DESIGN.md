# bsdeploy Design Document

## 1. Overview
`bsdeploy` is a command-line tool written in Rust designed to deploy applications to FreeBSD servers. It aims for simplicity and convention over configuration, managing system dependencies, environment variables, and application lifecycle.

## 2. Configuration
The deployment is defined in `config/bsdeploy.yml` at the root of the application repository.

### Example Configuration
```yaml
service: myapp

hosts:
  - bsd.localdomain

packages:
  - mise
  - libyaml

env:
  clear:
    - RAILS_ENV: production
  secret:
    - SECRET_KEY_BASE

before_start:
  - bin/rails assets:precompile
  - bin/rails db:migrate

start:
  - bin/rails server

data_directories:
  - /app/storage
```

### Data Structures
- **Config**: Root struct matching the YAML.
- **Env**: Struct with `clear` and `secret` maps/lists.

## 3. Architecture

### 3.1 CLI
Uses `clap` for argument parsing.
- Global flags: `--verbose`, `--config <path>` (defaulting to `config/bsdeploy.yml`).
- Subcommands: `setup`, `deploy`.

### 3.2 SSH & Remote Execution
The tool needs to execute commands on remote FreeBSD hosts.
- **Strategy**: Use the `ssh2` crate (libssh2 bindings) for programmatic control, or wrap the system `ssh` binary via `std::process::Command`.
- **Decision**: For the initial "simple" version, wrapping the system `ssh` and `scp`/`rsync` binaries is often more robust regarding SSH config (`~/.ssh/config`), agent forwarding, and keys than re-implementing auth logic with `ssh2`. However, for complex output handling, a library is better. We will start by wrapping `ssh` commands for simplicity and native config support.

### 3.3 File Transfer
- Use `rsync` (if available) or `scp` to transfer application code to the remote host.
- Default destination: `/var/db/bsdeploy/<service_name>` or similar.

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
**Goal**: Ship and run code.
1. **Ship**: `rsync` current directory to remote execution directory (excluding `.git`, `node_modules`, etc., via `.gitignore` or explicit exclude list).
2. **Prepare**:
    - Run `before_start` commands in sequence.
    - Set environment variables for these commands.
3. **Start**:
    - Run `start` commands.
    - **Process Management**: Initially, we might just run the command in the background or use `daemon(8)`. A simplified rc.d script generation or using `supervisord`/`pueue` could be a next step. For "start simple", we will likely use `daemon -p <pidfile> <command>`.

## 5. Future Considerations
- **Jails**: Encapsulate the application in a FreeBSD Jail (using `iocage` or raw `jail.conf`).
- **Zero-downtime**: Blue/Green deployments using Caddy to switch traffic.
- **Rollbacks**: Keep previous versions.
