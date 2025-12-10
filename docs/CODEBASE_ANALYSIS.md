# Kamal Codebase Analysis

## Overview

**Kamal** is a modern deployment tool created by Basecamp that enables zero-downtime deployment of containerized web applications to any server running Docker (from bare metal to cloud VMs). It uses [kamal-proxy](https://github.com/basecamp/kamal-proxy) to seamlessly manage request routing between container versions during deployment, ensuring zero downtime.

**Key Differentiator**: Unlike traditional container orchestration systems, Kamal is designed for simplicity and direct SSH-based deployments without requiring Kubernetes or other complex infrastructure.

Originally built for Rails applications, Kamal works with any web app that can be containerized with Docker.

## Technology Stack

### Primary Language and Framework

- **Language**: Ruby
- **CLI Framework**: Thor (for command-line interface)
- **Key Dependencies**:
  - `sshkit` (>= 1.23.0) - SSH protocol execution framework
  - `net-ssh` (7.3) - SSH client library
  - `activesupport` (>= 7.0) - Rails utilities library
  - `zeitwerk` (2.6.18+) - Ruby module autoloading
  - `concurrent-ruby` (1.2) - Concurrency utilities
  - `dotenv` (3.1) - Environment variable management
  - `bcrypt_pbkdf` & `ed25519` - SSH key support

## Architecture

### Three-Layer Design

1. **CLI Layer** (`Kamal::Cli`) - User-facing commands built with Thor
2. **Command Generation Layer** (`Kamal::Commands`) - Generates actual shell commands
3. **Configuration Layer** (`Kamal::Configuration`) - YAML parsing and validation

### Codebase Structure

```
/lib/kamal/
├── cli/                          # Command-line interface definitions (Thor-based)
│   ├── main.rb                   # Primary entry point with setup, deploy, rollback, etc.
│   ├── app.rb                    # App-specific commands (boot, start, stop, exec, logs)
│   ├── app/                      # App subcommands
│   │   ├── boot.rb              # Boots app containers
│   │   ├── assets.rb            # Asset management
│   │   ├── error_pages.rb       # Error page handling
│   │   └── ssl_certificates.rb  # SSL cert management
│   ├── build.rb                  # Build commands (deliver, push, pull, create)
│   ├── build/                    # Build subcommands
│   │   ├── clone.rb             # Git clone builder
│   │   └── port_forwarding.rb   # SSH port forwarding
│   ├── proxy.rb                  # kamal-proxy management commands
│   ├── accessory.rb              # Accessory service commands (db, redis, etc.)
│   ├── server.rb                 # Server bootstrap commands
│   ├── registry.rb               # Registry login/logout
│   ├── lock.rb                   # Deploy lock management
│   ├── prune.rb                  # Image and container cleanup
│   ├── secrets.rb                # Secret management helpers
│   ├── base.rb                   # Base class with lock, hook, SSH utilities
│   ├── templates/                # Config and secrets templates
│   └── healthcheck/              # Health check utilities
│
├── commands/                      # Low-level command generation (what gets executed)
│   ├── base.rb                   # Base command class with docker/ssh helpers
│   ├── app.rb                    # Docker commands for app containers
│   ├── app/                      # App command modules
│   │   ├── execution.rb          # Container execution commands
│   │   ├── containers.rb         # Container listing/management
│   │   ├── images.rb             # Image commands
│   │   ├── logging.rb            # Log streaming
│   │   ├── assets.rb             # Asset handling
│   │   ├── error_pages.rb        # Error page generation
│   │   └── proxy.rb              # Proxy integration
│   ├── builder.rb                # Image build orchestration
│   ├── builder/                  # Different builder strategies
│   │   ├── local.rb              # Local Docker build
│   │   ├── remote.rb             # Remote builder via SSH
│   │   ├── hybrid.rb             # Mix of local and remote
│   │   ├── pack.rb               # OCI packing
│   │   └── cloud.rb              # Cloud builder integration
│   ├── proxy.rb                  # kamal-proxy container commands
│   ├── accessory.rb              # Accessory container commands
│   ├── docker.rb                 # Docker command utilities
│   ├── registry.rb               # Registry login commands
│   ├── lock.rb                   # Deploy lock commands
│   ├── prune.rb                  # Cleanup commands
│   ├── auditor.rb                # Audit logging commands
│   └── hook.rb                   # Lifecycle hook execution
│
├── configuration/                 # Configuration parsing and validation
│   ├── validation.rb             # Validation framework
│   ├── validator/                # Validators for each config section
│   ├── accessory.rb              # Accessory config class
│   ├── proxy.rb                  # Proxy config class
│   ├── role.rb                   # Role (server group) config
│   ├── servers.rb                # Servers config
│   ├── builder.rb                # Builder config
│   ├── env.rb                    # Environment variables config
│   ├── registry.rb               # Registry config
│   ├── boot.rb                   # Boot/deployment config
│   ├── logging.rb                # Logging config
│   └── ssh.rb                    # SSH config
│
├── commander.rb                  # Central orchestrator (delegator pattern)
├── configuration.rb              # Main configuration loader (YAML parsing, ERB support)
├── secrets.rb                    # Secrets management from .kamal/secrets
├── docker.rb                     # Docker utility wrapper
├── git.rb                        # Git utilities
├── tags.rb                       # Version tagging
├── utils.rb                      # Helper functions
├── env_file.rb                   # Environment file generation
├── sshkit_with_ext.rb           # SSHKit extensions (DNS retry, command env merge)
└── version.rb                    # Version constant
```

## Key Components

### 1. CLI Layer (`Kamal::Cli`)

The command interface built with Thor. Key classes:
- `Kamal::Cli::Main` - Root commands (setup, deploy, redeploy, rollback, remove, upgrade)
- `Kamal::Cli::Base` - Shared functionality (hooks, locks, SSH, options parsing)
- Task-specific subcommands (App, Build, Proxy, Accessory, Server, Registry, etc.)

### 2. Command Generation Layer (`Kamal::Commands`)

Generates the actual shell commands to be executed. Examples:
- `docker run`, `docker inspect`, `docker ps`
- SSH commands via SSHKit
- Builder commands using `docker buildx`
- Registry commands

### 3. Configuration Layer (`Kamal::Configuration`)

Loads and validates `config/deploy.yml` and destination-specific files. Key aspects:
- YAML parsing with ERB template support
- Deep merge for destination-specific overrides (e.g., `deploy.staging.yml`)
- Validation against documented schema
- Support for multiple server roles, accessories, and proxies

### 4. Commander (Central Orchestrator)

`Kamal::Commander` - Singleton pattern class that:
- Holds the active configuration
- Creates command objects on demand
- Manages host/role filtering
- Tracks connection state and deploy locks
- Delegates to specialized command classes

### 5. SSH Integration (SSHKit)

Uses `sshkit` gem to:
- Execute commands over SSH on remote servers
- Support parallel execution
- Provide DSL for `on(hosts)` blocks
- Extended with DNS retry logic and command environment merging

## Deployment Technologies

### Docker-Based Architecture

- All applications must be containerized
- Uses Docker CLI commands (run, stop, inspect, etc.)
- Supports multi-architecture builds via `docker buildx`

### Builder Options

1. **Local Builder** - Build on local machine
2. **Remote Builder** - Build on remote SSH host
3. **Hybrid Builder** - Combine local and remote
4. **Pack Builder** - OCI packing for arm64 on x86
5. **Cloud Builder** - Integration with cloud providers

### Zero-Downtime Deployment via Proxy

- **kamal-proxy** container runs on each host
- Routes traffic to healthy containers
- Supports rolling deploys with configurable batch sizes
- Health checks with configurable intervals
- Automatic drain timeout before container stop

### Key Technologies

- **Containers**: Docker
- **SSH**: net-ssh, SSHKit
- **Load Balancing**: kamal-proxy (replaced Traefik in v2.0)
- **SSL/TLS**: Let's Encrypt auto-cert or custom certificates
- **Networking**: Docker networks (`kamal` and `kamal-proxy`)

## Configuration

### Main Config File: `config/deploy.yml`

```yaml
service: my-app                    # Container name prefix
image: registry/my-app            # Container image

servers:                           # Server roles
  web:
    - 192.168.0.1
  job:                            # Multiple roles per host possible
    cmd: bin/jobs

proxy:                            # kamal-proxy config
  ssl: true
  host: app.example.com

registry:                         # Container registry credentials
  server: docker.io

builder:                          # Build configuration
  arch: amd64

env:                              # Environment variables
  clear:
    DB_HOST: localhost
  secret:
    - RAILS_MASTER_KEY

volumes:                          # Persistent storage
  - app_storage:/app/storage

boot:                             # Rolling deploy config
  limit: 10
  wait: 2

accessories:                      # Supporting services
  db:
    image: mysql:8.0
    host: 192.168.0.2
    port: 3306
```

### Secrets File: `.kamal/secrets`

Environment variables that are sensitive, referenced in config via names

### Hooks: `.kamal/hooks/`

Scripts that run at lifecycle events:
- `pre-connect` - Before first SSH connection
- `pre-build` - Before building Docker image
- `pre-deploy` - Before deployment
- `post-deploy` - After deployment
- `pre-app-boot` - Before booting app containers
- `post-app-boot` - After booting app containers

## Main Deployment Workflows

### 1. Setup (`kamal setup`)

```
1. Bootstrap servers (install curl, Docker)
2. Build and push image
3. Boot kamal-proxy on all hosts
4. Boot accessories
5. Boot app containers
6. Tag image as latest
7. Prune old images
```

### 2. Deploy (`kamal deploy`)

```
1. Build and push image (or skip with --skip-push)
2. Boot kamal-proxy (if not running)
3. Boot accessories (optional)
4. Detect and stop stale containers
5. Boot new app containers
6. Route traffic via proxy to new containers
7. Prune old containers and images
```

### 3. Redeploy (`kamal redeploy`)

Lightweight deploy without:
- Proxy startup
- Accessory boot
- Full pruning

Useful for configuration-only changes

### 4. Rollback (`kamal rollback VERSION`)

```
1. Check if specified version exists as container
2. Route proxy to that version
3. Boot container if needed
```

### 5. Upgrade (`kamal upgrade`)

Migrates from Kamal 1.x (Traefik) to 2.0 (kamal-proxy)

## Command Structure

### Primary Commands (from `Kamal::Cli::Main`)

```bash
kamal setup              # Initial setup with proxy + app boot
kamal deploy             # Full deployment with image build
kamal redeploy           # Quick redeployment
kamal rollback VERSION   # Rollback to previous version
kamal details            # Show all container details
kamal audit              # Show deployment audit log
kamal config             # Display merged configuration
kamal remove             # Remove all deployed containers
kamal upgrade            # Upgrade from v1 to v2
kamal version            # Show version
kamal init               # Initialize config files
kamal docs [SECTION]     # Show configuration documentation
```

### Subcommands

- `kamal app` - boot, start, stop, restart, details, exec, logs, containers, remove, stale_containers
- `kamal build` - deliver, push, pull, create, remove, info, dev
- `kamal proxy` - boot, start, stop, details, logs, remove, upgrade, configure
- `kamal accessory` - boot, start, stop, restart, details, exec, logs, remove, upgrade, configure
- `kamal server` - bootstrap, ssh
- `kamal registry` - login, logout
- `kamal lock` - acquire, release, status, delete
- `kamal prune` - all, images, containers
- `kamal secrets` - extract, generate

### Global Options

```bash
-v, --verbose              Detailed logging
-q, --quiet                Minimal logging
-c, --config-file PATH     Config file location (default: config/deploy.yml)
-d, --destination NAME     Target environment (loads deploy.DESTINATION.yml)
--version VERSION          Run against specific version
-p, --primary              Run only on primary host
-h, --hosts LIST           Filter to specific hosts (comma-separated, wildcards)
-r, --roles LIST           Filter to specific roles (comma-separated, wildcards)
-H, --skip-hooks           Don't run lifecycle hooks
```

## Core Workflows and Logic

### CLI Command Flow

1. `bin/kamal` → `Kamal::Cli::Main.start(ARGV)`
2. Thor parses command and options
3. `Kamal::Cli::Base#initialize` - Parses options, configures Commander
4. Command method invokes other commands or runs hooks/locks
5. Commands call methods on `KAMAL` (global Commander instance)
6. Commander creates Command objects, executes via SSHKit

### App Deployment (`Kamal::Commands::App`)

- Includes: Assets, Containers, ErrorPages, Execution, Images, Logging, Proxy
- Generates Docker commands for running/stopping/inspecting containers
- Manages version extraction from container names
- Handles health checks

### Builder Orchestration (`Kamal::Commands::Builder`)

- Delegates to appropriate builder strategy (Local/Remote/Hybrid/Pack/Cloud)
- Manages builder creation/deletion via `docker buildx`
- Handles registry login and image push
- Supports no-cache builds

### Proxy Management (`Kamal::Commands::Proxy`)

- Manages kamal-proxy container lifecycle
- Configures SSL certificates (Let's Encrypt or custom)
- Handles traffic draining on stop
- Log streaming and version inspection

### Configuration Validation (`Kamal::Configuration`)

- Loads YAML with ERB template support
- Merges destination-specific configs
- Validates against schema
- Supports role-specific configurations
- Handles environment variable substitution

## Architectural Patterns

1. **Delegation Pattern**: Commander delegates to Command classes
2. **Strategy Pattern**: Builder has multiple implementations (Local/Remote/Hybrid/etc)
3. **Singleton Pattern**: KAMAL global Commander instance
4. **Module Mixins**: CLI and Command classes mix in functionality
5. **Configuration Objects**: Each config section is its own class
6. **SSH-based**: No agent required, pure SSH + Docker
7. **Idempotent Commands**: Safe to run multiple times
8. **Lock-based**: Deploy lock prevents concurrent deployments

## Dependency Flow

```
config/deploy.yml
  ↓
Kamal::Configuration (parses, validates, creates sub-configs)
  ↓
Kamal::Commander (holds config, creates commands on demand)
  ↓
CLI Commands (Kamal::Cli::*) - Thor-based user interface
  ↓
Commands (Kamal::Commands::*) - Generate shell commands
  ↓
SSHKit - Execute via SSH on remote hosts
```

## Notable Features

1. **Zero-Downtime Deployment**: kamal-proxy drains and switches traffic
2. **Multiple Roles**: Different commands per server (web/job/etc)
3. **Accessory Services**: Manage databases, caches, etc. alongside app
4. **Environment Management**: Clear/secret environment variables, per-host configs
5. **Rolling Deploys**: Configurable batch deployment with wait periods
6. **Hooks System**: Custom scripts at deployment lifecycle points
7. **Audit Logging**: Track all deployments on remote servers
8. **Asset Bridging**: Fingerprinted assets available across versions
9. **Multi-Destination**: Staging/production configs in one repo
10. **Git-based Versioning**: Uses git commit SHA as container version tag

## Key Design Principles

- **Simplicity over complexity**: Direct SSH + Docker, no orchestration overhead
- **Convention over configuration**: Sensible defaults, minimal required config
- **Zero-downtime deploys**: First-class support via kamal-proxy
- **Idempotent operations**: Safe to retry commands
- **Lock-based safety**: Prevents concurrent deploys
- **Parallel execution**: SSHKit enables concurrent remote operations
- **Extensibility**: Hooks for custom logic at lifecycle points

## Summary

Kamal is a well-architected, production-ready deployment tool that prioritizes simplicity and reliability over complex container orchestration. It's designed for teams that want zero-downtime deployments without the operational overhead of Kubernetes or similar platforms. The codebase demonstrates clean separation of concerns between user interface (CLI), business logic (Commands), and configuration management.
