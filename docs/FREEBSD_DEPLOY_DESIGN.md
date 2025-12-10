# FreeBSD Deployment Tool - Design Document

## Project Overview

A Rust-based deployment tool for FreeBSD systems, inspired by Kamal but tailored for FreeBSD-native deployment without containers. The tool focuses on simplicity, zero-downtime deployments, and leveraging FreeBSD's native capabilities.

**Working Name**: `bsdeploy` (subject to change)

### Goals

- **Zero-downtime deployments** for Rails applications on FreeBSD
- **User isolation**: Each application runs as its own system user
- **Simple configuration**: Single TOML file for deployment config
- **Push-based**: Deploy from local checkout to remote server
- **FreeBSD-native**: No containers, use native FreeBSD features
- **Production-ready**: Built for reliability and safety

### Non-Goals (for v1)

- FreeBSD jails (planned for later)
- Multi-server orchestration (single server initially)
- Docker/container support
- Database management (apps use SQLite initially)
- Complex service dependencies

## Architecture

### High-Level Design

```
┌─────────────────┐
│   CLI (Rust)    │ ← User runs commands
└────────┬────────┘
         │
         ├─ Config Parser (TOML)
         │
         ├─ SSH Client
         │
         ├─ Command Generator
         │
         └─ State Manager
              │
              ▼
    ┌─────────────────────┐
    │  FreeBSD Host       │
    │                     │
    │  ┌──────────────┐  │
    │  │ Caddy Proxy  │  │ ← Routes traffic
    │  └──────┬───────┘  │
    │         │           │
    │  ┌──────▼───────┐  │
    │  │ App Instance │  │ ← Runs as user 'myapp'
    │  │ (Rails/Puma) │  │
    │  └──────────────┘  │
    └─────────────────────┘
```

### Component Architecture

```rust
bsdeploy/
├── CLI Layer
│   ├── Command parsing (clap)
│   ├── User interaction
│   └── Output formatting
│
├── Configuration Layer
│   ├── TOML parsing (serde)
│   ├── Validation
│   └── Secrets management
│
├── Execution Layer
│   ├── SSH client wrapper
│   ├── Command builder
│   └── Output capture
│
├── FreeBSD Layer
│   ├── User management (pw)
│   ├── Package management (pkg)
│   ├── Service management (rc.d)
│   └── File system operations
│
├── Application Layer
│   ├── Source transfer (git archive push)
│   ├── Ruby/Bundler
│   ├── Asset compilation
│   └── Database migrations
│
├── Proxy Layer
│   ├── Caddy configuration
│   ├── Health checks
│   └── Traffic switching
│
└── Runtime Layer
    ├── Version management
    ├── Installation strategies
    └── Environment setup
```

### Runtime Management Architecture

The tool uses a **strategy pattern** to handle different runtime environments (Ruby, Node, Python, etc.) without requiring a plugin architecture in v1.

```rust
// Runtime Strategy Pattern
pub trait RuntimeStrategy {
    fn name(&self) -> &str;
    fn detect_version(&self, ssh: &SshClient) -> Result<Option<String>>;
    fn install(&self, ssh: &SshClient, version: &str) -> Result<()>;
    fn setup_environment(&self, version: &str) -> HashMap<String, String>;
    fn get_executable_path(&self, version: &str) -> String;
}

// Installation Flow
┌─────────────────────────────────────────┐
│  Parse runtime config (ruby: "3.3.0")  │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Get RuntimeStrategy for "ruby"         │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Check if version already installed     │
└──────────────────┬──────────────────────┘
                   │
        ┌──────────┴──────────┐
        │                     │
        ▼                     ▼
   [Installed]          [Not Installed]
        │                     │
        │                     ▼
        │         ┌───────────────────────┐
        │         │ Try pkg install first │
        │         └──────────┬────────────┘
        │                    │
        │         ┌──────────┴──────────┐
        │         │                     │
        │         ▼                     ▼
        │    [Available]          [Not Available]
        │         │                     │
        │         ▼                     ▼
        │   [Install via pkg]   [Install via version manager]
        │                              │
        │                              ├─ ruby-install + chruby
        │                              ├─ nvm (node)
        │                              └─ pyenv (python)
        │                              │
        └──────────────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Setup environment variables (PATH)     │
└─────────────────────────────────────────┘
```

**Key Design Decisions:**
- **Fallback Strategy**: Try system packages first, fall back to version managers
- **Per-User Installation**: Runtimes installed in user home directory (`/home/myapp/.rubies/`)
- **Version Managers**: Leverage existing tools (ruby-install, nvm, pyenv)
- **No Compilation Caching**: Install from source each time (v2 could add caching)

**Supported Runtimes (v1):**
- Ruby: via `pkg` or `ruby-install` + `chruby`
- Node: via `pkg` or `nvm`

**Future Runtimes (v2+):**
- Python: via `pkg` or `pyenv`
- Go: via `pkg` or manual installation
- Custom: plugin architecture for user-defined runtimes

## Technology Stack

### Core Technologies

- **Language**: Rust (stable)
- **CLI Framework**: `clap` v4 (derive API)
- **SSH**: `russh` or `async-ssh2-tokio`
- **Async Runtime**: `tokio`
- **Configuration**: `serde` + `toml`
- **Error Handling**: `anyhow` + `thiserror`
- **Logging**: `tracing` + `tracing-subscriber`
- **HTTP Client**: `reqwest` (for Caddy API)

### FreeBSD Tools

- **Package Manager**: `pkg`
- **User Management**: `pw` command
- **Service Management**: `service` command + rc.d scripts
- **Web Server**: Caddy
- **Process Manager**: Built-in rc.d system

## Configuration Format

The configuration will live inside the project we intend to deploy. Its location is config/deploy.toml by default.

### Main Config: `deploy.toml`

```toml
[app]
name = "myapp"                              # Used for user, directory, service name

[runtime]
ruby = "3.3.0"                              # Tries pkg first, falls back to ruby-install
node = "20.11.0"                            # Tries pkg first, falls back to nvm

[server]
host = "192.168.1.100"                      # FreeBSD server hostname/IP
user = "deploy"                             # SSH user (must have sudo)
port = 22                                   # SSH port (optional, default 22)
ssh_key = "~/.ssh/id_ed25519"              # SSH private key path

[deployment]
app_dir = "/home/{{ app_name }}/app"       # Base directory (templated)
keep_releases = 5                           # Number of releases to keep (optional, default 3)
shared_dirs = ["log", "tmp", "storage", "db"]  # Shared across releases

[web]
domain = "myapp.example.com"                # Primary domain
port = 3000                                 # App listen port (or use socket)
process_count = 2                           # Number of app processes
health_check_path = "/up"                   # Health check endpoint
health_check_timeout = 30                   # Seconds to wait for health

[env]
# Clear environment variables
RAILS_ENV = "production"
RAILS_LOG_TO_STDOUT = "1"

# Secret variables (loaded from deploy.secrets.toml)
[env.secret]
keys = ["RAILS_MASTER_KEY", "SECRET_KEY_BASE"]

[hooks]
# Optional hook scripts (local paths, uploaded and executed)
pre_deploy = ".deploy/hooks/pre_deploy.sh"
post_deploy = ".deploy/hooks/post_deploy.sh"
```

### Secrets Config: `deploy.secrets.toml` (gitignored)

```toml
RAILS_MASTER_KEY = "abc123..."
SECRET_KEY_BASE = "xyz789..."
```

## Directory Structure on FreeBSD Host

```
/home/myapp/                          # App user home
├── app/                              # Deployment root
│   ├── current -> releases/20250109_143022/  # Symlink to active
│   ├── releases/                     # Timestamped releases
│   │   ├── 20250109_143022/         # Release directory
│   │   │   ├── app/                 # Rails app code
│   │   │   ├── Gemfile
│   │   │   ├── Gemfile.lock
│   │   │   └── ...
│   │   ├── 20250109_120000/         # Previous release
│   │   └── ...
│   └── shared/                       # Shared across releases
│       ├── log/                      # Application logs
│       ├── tmp/                      # Temp files
│       │   ├── pids/
│       │   └── sockets/
│       ├── storage/                  # Active Storage files
│       └── db/                       # SQLite database
│           └── production.sqlite3
├── .rubies/                          # Ruby installations (via ruby-install)
│   ├── ruby-3.3.0/
│   │   ├── bin/
│   │   └── lib/
│   └── ...
├── .nvm/                             # Node.js installations (via nvm)
│   └── versions/
│       └── node/
│           └── v20.11.0/
└── .ssh/                             # SSH keys if needed
    └── authorized_keys
```

## Core Workflows

### 1. Initial Setup (`bsdeploy setup`)

**Purpose**: Prepare a fresh FreeBSD server for the application

**Steps**:
1. **Verify SSH Connection**
   - Test SSH connectivity to server
   - Verify sudo access

2. **Install System Dependencies**
   ```bash
   pkg install -y ruby33 node20 git caddy sqlite3
   ```

3. **Create App User**
   ```bash
   pw useradd myapp -m -s /bin/sh -c "MyApp Service Account"
   ```

4. **Setup Directory Structure**
   ```bash
   mkdir -p /home/myapp/app/{releases,shared/{log,tmp,storage,db}}
   chown -R myapp:myapp /home/myapp/app
   ```

5. **Push Application Source** (first release)
   ```bash
   # bsdeploy creates release directory and pushes source from local checkout
   mkdir -p /home/myapp/app/releases/20250109_143022
   # Source is pushed via git archive from local machine
   # (executed by bsdeploy tool internally)
   ```

6. **Install Dependencies**
   ```bash
   bundle config set --local deployment true
   bundle config set --local without 'development test'
   bundle install
   ```

7. **Setup Shared Directories**
   ```bash
   ln -sf ../../shared/log log
   ln -sf ../../shared/tmp tmp
   ln -sf ../../shared/storage storage
   ln -sf ../../shared/db db
   ```

8. **Database Setup**
   ```bash
   RAILS_ENV=production bin/rails db:create db:migrate
   ```

9. **Asset Precompilation**
   ```bash
   RAILS_ENV=production bin/rails assets:precompile
   ```

10. **Create rc.d Service Script**
    ```bash
    # /usr/local/etc/rc.d/myapp
    # Service script to start/stop the app
    ```

11. **Enable and Start Service**
    ```bash
    sysrc myapp_enable="YES"
    service myapp start
    ```

12. **Configure Caddy**
    - Add reverse proxy configuration
    - Setup automatic HTTPS
    - Reload Caddy

13. **Create Current Symlink**
    ```bash
    ln -sf releases/20250109_143022 /home/myapp/app/current
    ```

**Success**: Application is running and accessible via HTTPS

### 2. Standard Deployment (`bsdeploy deploy`)

**Purpose**: Deploy a new version with zero downtime

**Steps**:
1. **Create New Release Directory**
   ```bash
   RELEASE=$(date +%Y%m%d_%H%M%S)
   mkdir /home/myapp/app/releases/$RELEASE
   ```

2. **Push Source Code**
   ```bash
   # bsdeploy pushes source from local checkout to release directory
   # Source transferred via git archive from local machine
   # (executed by bsdeploy tool internally)
   ```

3. **Link Shared Directories**
   ```bash
   ln -sf ../../shared/log log
   ln -sf ../../shared/tmp tmp
   ln -sf ../../shared/storage storage
   ln -sf ../../shared/db db
   ```

4. **Install Dependencies**
   ```bash
   bundle install --deployment --without development test
   ```

5. **Asset Precompilation**
   ```bash
   RAILS_ENV=production bin/rails assets:precompile
   ```

6. **Database Migration**
   ```bash
   RAILS_ENV=production bin/rails db:migrate
   ```

7. **Pre-deploy Hooks** (if configured)
   ```bash
   ./pre_deploy.sh
   ```

8. **Start New Process** (parallel to old)
   - Update rc.d script to point to new release
   - Start new process on different port/socket
   - Wait for process to be ready

9. **Health Check**
   - Poll health check endpoint
   - Timeout if not healthy within configured time
   - Rollback if unhealthy

10. **Update Caddy Configuration**
    - Switch reverse proxy to new process
    - Reload Caddy (graceful)

11. **Drain Old Process**
    - Wait for existing connections to complete
    - Stop old process

12. **Update Current Symlink**
    ```bash
    ln -sfn releases/$RELEASE /home/myapp/app/current
    ```

13. **Cleanup Old Releases**
    - Keep only last N releases (configured)
    - Remove old release directories

14. **Post-deploy Hooks** (if configured)
    ```bash
    ./post_deploy.sh
    ```

**Success**: New version is live, old version cleanly shut down

### 3. Rollback (`bsdeploy rollback [VERSION]`)

**Purpose**: Quickly revert to a previous release

**Steps**:
1. **Identify Target Release**
   - Use specified version OR previous release
   - Verify release exists

2. **Health Check Old Release**
   - Ensure old release is intact
   - Verify database compatibility

3. **Start Old Process**
   - Update rc.d script to old release
   - Start process

4. **Health Check**
   - Verify old version is healthy

5. **Update Caddy**
   - Switch to old process
   - Reload Caddy

6. **Stop Current Process**
   - Graceful shutdown

7. **Update Current Symlink**
   ```bash
   ln -sfn releases/$OLD_RELEASE /home/myapp/app/current
   ```

**Success**: Running previous version

### 4. Status Check (`bsdeploy status`)

**Purpose**: Display current deployment state

**Output**:
- Current release version and timestamp
- Running processes (ps output)
- Caddy status and routing
- Recent releases available
- Health check status

## Rust Project Structure

```
bsdeploy/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
│
├── src/
│   ├── main.rs                    # CLI entry point
│   │
│   ├── cli/
│   │   ├── mod.rs                 # CLI module exports
│   │   ├── setup.rs               # `bsdeploy setup` command
│   │   ├── deploy.rs              # `bsdeploy deploy` command
│   │   ├── rollback.rs            # `bsdeploy rollback` command
│   │   ├── status.rs              # `bsdeploy status` command
│   │   └── init.rs                # `bsdeploy init` (create deploy.toml)
│   │
│   ├── config/
│   │   ├── mod.rs
│   │   ├── deploy.rs              # DeployConfig struct + parsing
│   │   ├── secrets.rs             # Secrets loading
│   │   └── validation.rs          # Config validation
│   │
│   ├── ssh/
│   │   ├── mod.rs                 # SSH client wrapper
│   │   ├── client.rs              # Connection management
│   │   └── executor.rs            # Command execution helpers
│   │
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── freebsd.rs             # FreeBSD system commands
│   │   │                          # - pw (user management)
│   │   │                          # - pkg (package management)
│   │   │                          # - service (service management)
│   │   ├── source_transfer.rs     # Source code push operations (git archive)
│   │   ├── bundler.rs             # Bundler operations
│   │   ├── rails.rs               # Rails commands (assets, migrations)
│   │   ├── caddy.rs               # Caddy configuration
│   │   └── rcd.rs                 # rc.d service script generation
│   │
│   ├── runtime/
│   │   ├── mod.rs                 # Runtime strategy trait and registry
│   │   ├── ruby.rs                # Ruby installation (pkg + ruby-install)
│   │   └── node.rs                # Node installation (pkg + nvm)
│   │
│   ├── deployment/
│   │   ├── mod.rs
│   │   ├── release.rs             # Release management
│   │   ├── health.rs              # Health check logic
│   │   └── rollback.rs            # Rollback logic
│   │
│   ├── templates/
│   │   ├── mod.rs
│   │   ├── rcd_script.rs          # rc.d service template
│   │   ├── caddyfile.rs           # Caddy config template
│   │   └── deploy_toml.rs         # deploy.toml template (for init)
│   │
│   ├── error.rs                   # Error types (thiserror)
│   └── lib.rs                     # Library exports
│
└── tests/
    ├── integration/               # Integration tests
    │   ├── setup_test.rs
    │   └── deploy_test.rs
    └── fixtures/                  # Test fixtures
        └── sample_deploy.toml
```

## Key Rust Types

### Configuration Types

```rust
// src/config/deploy.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize)]
pub struct DeployConfig {
    pub app: AppConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    pub deployment: DeploymentConfig,
    pub web: WebConfig,
    pub env: EnvConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AppConfig {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct RuntimeConfig {
    pub ruby: Option<String>,
    pub node: Option<String>,
    // Future: python, go, etc.
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub user: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub ssh_key: Option<String>,
}

fn default_ssh_port() -> u16 { 22 }

#[derive(Debug, Deserialize, Serialize)]
pub struct DeploymentConfig {
    pub app_dir: String,
    #[serde(default = "default_keep_releases")]
    pub keep_releases: usize,
    pub shared_dirs: Vec<String>,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,  // Additional patterns to exclude beyond .gitignore
}

fn default_keep_releases() -> usize { 5 }

#[derive(Debug, Deserialize, Serialize)]
pub struct WebConfig {
    pub domain: String,
    pub port: u16,
    #[serde(default = "default_process_count")]
    pub process_count: usize,
    #[serde(default = "default_health_path")]
    pub health_check_path: String,
    #[serde(default = "default_health_timeout")]
    pub health_check_timeout: u64,
}

fn default_process_count() -> usize { 2 }
fn default_health_path() -> String { "/up".to_string() }
fn default_health_timeout() -> u64 { 30 }

#[derive(Debug, Deserialize, Serialize)]
pub struct EnvConfig {
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub secret: SecretEnvConfig,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SecretEnvConfig {
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct HooksConfig {
    pub pre_deploy: Option<String>,
    pub post_deploy: Option<String>,
}
```

### SSH Types

```rust
// src/ssh/mod.rs
use anyhow::Result;

pub struct SshClient {
    host: String,
    port: u16,
    user: String,
    key_path: Option<String>,
    // Internal SSH session
}

impl SshClient {
    pub async fn connect(config: &ServerConfig) -> Result<Self>;
    pub async fn exec(&self, command: &str) -> Result<CommandOutput>;
    pub async fn exec_with_env(&self, command: &str, env: &HashMap<String, String>) -> Result<CommandOutput>;
    pub async fn upload_file(&self, local: &Path, remote: &Path) -> Result<()>;
    pub async fn exec_with_stdin(&self, command: &str, stdin: impl Read) -> Result<CommandOutput>;
}

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    pub fn ensure_success(&self) -> Result<&Self>;
}
```

### Command Builder Types

```rust
// src/commands/mod.rs

pub trait CommandBuilder {
    fn build(&self) -> String;
}

// Example: FreeBSD user management
pub struct CreateUserCommand {
    pub username: String,
    pub home_dir: Option<String>,
    pub shell: String,
    pub comment: String,
}

impl CommandBuilder for CreateUserCommand {
    fn build(&self) -> String {
        format!(
            "pw useradd {} -m -s {} -c '{}'",
            self.username,
            self.shell,
            self.comment
        )
    }
}
```

### Source Transfer Types

```rust
// src/commands/source_transfer.rs
use anyhow::Result;
use std::path::Path;

pub struct SourceTransfer {
    ssh: SshClient,
}

impl SourceTransfer {
    /// Push source code from local checkout to remote release directory
    pub async fn push(&self, local_path: &Path, remote_path: &Path, git_ref: Option<&str>) -> Result<String> {
        // Returns git SHA of deployed commit
    }

    /// Detect if current directory is a git repository
    pub fn is_git_repo(path: &Path) -> Result<bool>;

    /// Get current git SHA
    pub fn get_git_sha(path: &Path, git_ref: Option<&str>) -> Result<String>;

    /// Check for uncommitted changes
    pub fn has_uncommitted_changes(path: &Path) -> Result<bool>;

    /// Push using git archive (preferred)
    async fn push_git_archive(&self, local_path: &Path, remote_path: &Path, git_ref: &str) -> Result<()>;

    /// Fallback push using tar (for non-git directories)
    async fn push_tar(&self, local_path: &Path, remote_path: &Path, exclude: &[String]) -> Result<()>;
}
```

### Runtime Strategy Types

```rust
// src/runtime/mod.rs
use anyhow::Result;
use std::collections::HashMap;

/// Strategy trait for runtime installation and management
pub trait RuntimeStrategy: Send + Sync {
    /// Runtime name (e.g., "ruby", "node")
    fn name(&self) -> &str;

    /// Detect if a specific version is already installed
    fn detect_version(&self, ssh: &SshClient, version: &str) -> Result<bool>;

    /// Install a specific version (tries pkg first, falls back to version manager)
    fn install(&self, ssh: &SshClient, version: &str, app_user: &str) -> Result<()>;

    /// Get environment variables needed to use this runtime version
    fn setup_environment(&self, version: &str, app_user: &str) -> HashMap<String, String>;
}

// Example: Ruby strategy
pub struct RubyStrategy;

impl RuntimeStrategy for RubyStrategy {
    fn name(&self) -> &str { "ruby" }

    fn detect_version(&self, ssh: &SshClient, version: &str) -> Result<bool> {
        // Check if Ruby version is installed
        let ruby_path = format!("/home/{}/.rubies/ruby-{}/bin/ruby", app_user, version);
        let output = ssh.exec(&format!("test -f {} && echo 'exists'", ruby_path))?;
        Ok(output.stdout.trim() == "exists")
    }

    fn install(&self, ssh: &SshClient, version: &str, app_user: &str) -> Result<()> {
        // Try pkg first
        if self.try_pkg_install(ssh, version)? {
            return Ok(());
        }

        // Fall back to ruby-install
        self.install_via_ruby_install(ssh, version, app_user)
    }

    fn setup_environment(&self, version: &str, app_user: &str) -> HashMap<String, String> {
        let mut env = HashMap::new();
        let ruby_bin = format!("/home/{}/.rubies/ruby-{}/bin", app_user, version);
        env.insert("PATH".to_string(),
                   format!("{}:/usr/local/bin:/usr/bin:/bin", ruby_bin));
        env
    }
}

impl RubyStrategy {
    fn try_pkg_install(&self, ssh: &SshClient, version: &str) -> Result<bool> {
        // Try to install via pkg (e.g., ruby33 for version 3.3)
        let major_minor = version.split('.').take(2).collect::<Vec<_>>().join("");
        let pkg_name = format!("ruby{}", major_minor);

        // Check if package exists
        let output = ssh.exec(&format!("pkg search -q '^{}$'", pkg_name))?;
        if output.stdout.trim().is_empty() {
            return Ok(false);
        }

        // Install it
        ssh.exec(&format!("pkg install -y {}", pkg_name))?;
        Ok(true)
    }

    fn install_via_ruby_install(&self, ssh: &SshClient, version: &str, app_user: &str) -> Result<()> {
        // Install ruby-install and chruby if not present
        ssh.exec("pkg install -y ruby-install chruby")?;

        // Install Ruby as the app user
        ssh.exec(&format!(
            "su - {} -c 'ruby-install ruby {}'",
            app_user, version
        ))?;

        Ok(())
    }
}

// Registry for runtime strategies
pub struct RuntimeRegistry {
    strategies: HashMap<String, Box<dyn RuntimeStrategy>>,
}

impl RuntimeRegistry {
    pub fn new() -> Self {
        let mut strategies: HashMap<String, Box<dyn RuntimeStrategy>> = HashMap::new();
        strategies.insert("ruby".to_string(), Box::new(RubyStrategy));
        strategies.insert("node".to_string(), Box::new(NodeStrategy));

        Self { strategies }
    }

    pub fn get(&self, runtime: &str) -> Option<&dyn RuntimeStrategy> {
        self.strategies.get(runtime).map(|s| s.as_ref())
    }
}
```

## Implementation Plan

### Phase 0: Project Setup (1-2 days)

**Goal**: Initialize Rust project with basic structure

**Tasks**:
1. ✅ Create new Rust project with Cargo
   - `cargo new bsdeploy --bin`
   - Add initial dependencies to Cargo.toml

2. ✅ Setup project structure
   - Create module directories
   - Add README.md with project description
   - Add LICENSE file

3. ✅ Configure development tools
   - Setup rustfmt configuration
   - Setup clippy lints
   - Add .gitignore

4. ✅ Create basic CLI with clap
   - Define command structure (setup, deploy, rollback, status, init)
   - Add --help output
   - Parse basic arguments

**Deliverable**: Project compiles and shows help message

### Phase 1: Configuration (2-3 days)

**Goal**: Load and validate deployment configuration

**Tasks**:
1. ✅ Define configuration structs
   - AppConfig
   - ServerConfig
   - DeploymentConfig
   - WebConfig
   - EnvConfig

2. ✅ Implement TOML parsing
   - Load deploy.toml
   - Handle missing optional fields
   - Use serde defaults

3. ✅ Add validation
   - Validate required fields
   - Check repository URL format
   - Validate port numbers
   - Check directory paths

4. ✅ Implement secrets loading
   - Load deploy.secrets.toml
   - Merge with main config
   - Ensure secrets file is optional

5. ✅ Add `bsdeploy init` command
   - Generate sample deploy.toml
   - Generate sample deploy.secrets.toml
   - Create .gitignore entry

**Deliverable**: Can parse and validate config files

**Test**:
```bash
bsdeploy init
bsdeploy config validate
```

### Phase 2: SSH Connection (2-3 days)

**Goal**: Establish SSH connection and execute remote commands

**Tasks**:
1. ✅ Choose SSH library
   - Evaluate russh vs async-ssh2-tokio
   - Add to dependencies

2. ✅ Implement SshClient
   - Connection establishment
   - Key-based authentication
   - Password authentication (optional)

3. ✅ Implement command execution
   - Execute single command
   - Capture stdout/stderr
   - Return exit code

4. ✅ Add error handling
   - Connection failures
   - Command failures
   - Timeout handling

5. ✅ Add logging
   - Log commands being executed
   - Log command output (optional verbose mode)

6. ✅ Test SSH connectivity
   - Add `bsdeploy ssh test` command
   - Verify connection works

**Deliverable**: Can SSH to server and run commands

**Test**:
```bash
bsdeploy ssh test
bsdeploy ssh exec "uname -a"
```

### Phase 3: FreeBSD Command Builders (2-3 days)

**Goal**: Generate FreeBSD-specific commands

**Tasks**:
1. ✅ Implement user management commands
   - `pw useradd` - Create user
   - `pw usermod` - Modify user
   - `pw userdel` - Delete user
   - `id` - Check if user exists

2. ✅ Implement package management commands
   - `pkg install` - Install packages
   - `pkg info` - Check if package installed
   - `pkg update` - Update package database

3. ✅ Implement service management commands
   - `service start/stop/restart`
   - `sysrc` - Enable/disable services
   - Service status checks

4. ✅ Implement file system commands
   - Directory creation (mkdir -p)
   - Symlink management
   - Permission changes (chown)
   - File existence checks

5. ✅ Add command builder tests
   - Unit tests for each command type
   - Verify command string format

**Deliverable**: Command generation library

**Test**: Unit tests pass

### Phase 4: Source Transfer (1-2 days)

**Goal**: Push application source from local checkout to remote server

**Tasks**:
1. ✅ Implement git archive-based push
   - Detect current git repository (working directory)
   - Execute `git archive` to create clean tarball of HEAD
   - Stream compressed archive over SSH connection
   - Extract directly in remote release directory
   - Atomic single-operation transfer

2. ✅ Add commit/ref specification
   - Default to HEAD (current commit)
   - Support --ref flag for specific commits/tags/branches
   - Validate ref exists before transfer

3. ✅ Add local verification
   - Verify working directory is a git repository
   - Check for uncommitted changes (warn user)
   - Capture git SHA for deployment tracking

4. ✅ Implement fallback for non-git directories
   - Detect when not in git repository
   - Use tar with exclude patterns
   - Respect custom exclude_patterns from config

5. ✅ Optimize transfer efficiency
   - Stream with gzip compression
   - No intermediate temp files
   - Progress bar for large transfers

**Deliverable**: `bsdeploy` can push source code efficiently from local checkout to remote server

**Internal implementation** (what bsdeploy executes):
```rust
// Pseudo-code for the Rust implementation
pub async fn push_source(ssh: &SshClient, local_path: &Path, remote_path: &Path) -> Result<()> {
    // Create git archive and stream over SSH
    let archive = Command::new("git")
        .args(&["archive", "--format=tar", "HEAD"])
        .stdout(Stdio::piped())
        .spawn()?;

    // Compress and send via SSH
    ssh.exec_piped(&format!("mkdir -p {} && tar -xzf - -C {}", remote_path, remote_path),
                   archive.stdout)?;
}
```

### Phase 5: Rails Application Commands (2-3 days)

**Goal**: Execute Rails-specific commands

**Tasks**:
1. ✅ Implement bundler commands
   - `bundle install`
   - `bundle config`
   - Check if dependencies changed (Gemfile.lock)

2. ✅ Implement asset precompilation
   - `rails assets:precompile`
   - Detect if assets changed
   - Clean old assets

3. ✅ Implement database migrations
   - `rails db:migrate`
   - `rails db:migrate:status`
   - Rollback support

4. ✅ Implement database setup
   - `rails db:create`
   - `rails db:schema:load`

5. ✅ Add Rails environment handling
   - Set RAILS_ENV
   - Load environment variables

**Deliverable**: Can run Rails commands remotely

### Phase 5.5: Runtime Management (2-3 days)

**Goal**: Install and manage runtime versions (Ruby, Node)

**Tasks**:
1. ✅ Define RuntimeStrategy trait
   - Version detection
   - Installation method
   - Environment setup
   - Trait for extensibility

2. ✅ Implement RubyStrategy
   - Detect installed Ruby versions
   - Try pkg installation first
   - Fall back to ruby-install + chruby
   - Setup PATH and environment

3. ✅ Implement NodeStrategy
   - Detect installed Node versions
   - Try pkg installation first
   - Fall back to nvm
   - Setup PATH and environment

4. ✅ Add RuntimeRegistry
   - Register available strategies
   - Lookup strategies by name
   - Easy to add new runtimes

5. ✅ Integrate with setup workflow
   - Install runtimes before app deployment
   - Verify installation success
   - Cache detection results

6. ✅ Add runtime configuration parsing
   - Parse RuntimeConfig from TOML
   - Validate version strings
   - Handle optional runtimes

7. ✅ Add tests
   - Test version detection
   - Test pkg vs version manager fallback
   - Test environment generation

**Deliverable**: Can install Ruby and Node versions automatically

**Test**:
```bash
# In deploy.toml:
# [runtime]
# ruby = "3.3.0"
# node = "20.11.0"

bsdeploy setup
# Should install both runtimes and verify they work
```

### Phase 6: rc.d Service Management (2-3 days)

**Goal**: Generate and manage rc.d service scripts

**Tasks**:
1. ✅ Create rc.d script template
   - FreeBSD rc.d format
   - Start/stop/restart commands
   - PID file management
   - Environment variable support

2. ✅ Implement template rendering
   - Replace placeholders (app name, paths, etc.)
   - Generate script from config

3. ✅ Implement service deployment
   - Upload script to /usr/local/etc/rc.d/
   - Set executable permissions
   - Enable service with sysrc

4. ✅ Implement service control
   - Start service
   - Stop service
   - Restart service
   - Check status

5. ✅ Add multi-process support
   - Run multiple app instances
   - Separate PID files
   - Port/socket allocation

**Deliverable**: Can create and manage rc.d services

### Phase 7: Caddy Integration (3-4 days)

**Goal**: Configure Caddy reverse proxy

**Tasks**:
1. ✅ Create Caddyfile template
   - Reverse proxy configuration
   - Automatic HTTPS
   - Health check integration
   - Load balancing (multiple processes)

2. ✅ Implement Caddy configuration
   - Generate Caddyfile from config
   - Upload to server
   - Validate syntax

3. ✅ Implement Caddy control
   - Install Caddy (pkg install)
   - Start/stop Caddy
   - Reload configuration
   - Check Caddy status

4. ✅ Add traffic switching
   - Update upstream targets
   - Graceful reload
   - Zero-downtime switching

5. ✅ Implement health checks
   - HTTP health check requests
   - Retry logic
   - Timeout handling

**Deliverable**: Can configure and control Caddy

### Phase 8: Setup Command (3-4 days)

**Goal**: Implement `bsdeploy setup` for initial deployment

**Tasks**:
1. ✅ Implement system dependency installation
   - Check if packages installed
   - Install Ruby, Node, Git, Caddy, SQLite
   - Verify installation

2. ✅ Implement user creation
   - Create app user
   - Setup home directory
   - Set permissions

3. ✅ Implement directory structure setup
   - Create releases, shared directories
   - Create shared subdirectories
   - Set ownership

4. ✅ Implement first deployment
   - Clone repository
   - Create timestamped release
   - Install dependencies
   - Setup shared directory symlinks

5. ✅ Implement database setup
   - Run db:create
   - Run db:migrate
   - Verify database

6. ✅ Implement asset compilation
   - Precompile assets
   - Verify output

7. ✅ Implement service setup
   - Generate rc.d script
   - Enable service
   - Start service

8. ✅ Implement Caddy setup
   - Generate Caddyfile
   - Configure Caddy
   - Start Caddy

9. ✅ Create current symlink
   - Link to first release

10. ✅ Add comprehensive error handling
    - Rollback on failure
    - Clear error messages
    - Cleanup partial state

**Deliverable**: Complete working `bsdeploy setup` command

**Test**: Deploy a Rails app from scratch

### Phase 9: Deploy Command (4-5 days)

**Goal**: Implement `bsdeploy deploy` for zero-downtime updates

**Tasks**:
1. ✅ Implement release creation
   - Generate timestamp
   - Create release directory
   - Clone/pull code

2. ✅ Implement dependency installation
   - Compare Gemfile.lock
   - Run bundle install if needed
   - Optimize for speed

3. ✅ Implement asset compilation
   - Detect asset changes
   - Precompile if needed
   - Skip if unchanged

4. ✅ Implement database migration
   - Run migrations
   - Handle migration failures

5. ✅ Implement new process startup
   - Start new app processes
   - Different ports/sockets
   - Parallel to old processes

6. ✅ Implement health checking
   - Poll health endpoint
   - Configurable timeout
   - Fail deployment if unhealthy

7. ✅ Implement traffic switching
   - Update Caddy configuration
   - Reload Caddy
   - Verify routing

8. ✅ Implement old process shutdown
   - Graceful shutdown
   - Wait for connections to drain
   - Force kill if necessary

9. ✅ Update current symlink
   - Atomic symlink update

10. ✅ Implement cleanup
    - Remove old releases
    - Keep configured number

11. ✅ Add hooks support
    - Pre-deploy hook
    - Post-deploy hook
    - Error handling

**Deliverable**: Complete working `bsdeploy deploy` command

**Test**: Deploy multiple versions with zero downtime

### Phase 10: Rollback Command (2-3 days)

**Goal**: Implement `bsdeploy rollback` for quick reversion

**Tasks**:
1. ✅ Implement release listing
   - List available releases
   - Show current release
   - Show previous release

2. ✅ Implement version selection
   - Rollback to previous (default)
   - Rollback to specific version
   - Validate version exists

3. ✅ Implement process restart
   - Start old version processes
   - Health check

4. ✅ Implement traffic switching
   - Update Caddy to old version
   - Reload Caddy

5. ✅ Stop current processes
   - Shutdown current version

6. ✅ Update current symlink
   - Point to old release

7. ✅ Add database rollback warning
   - Warn about incompatible migrations
   - Require confirmation

**Deliverable**: Working `bsdeploy rollback` command

**Test**: Deploy, then rollback

### Phase 11: Status Command (1-2 days)

**Goal**: Implement `bsdeploy status` for visibility

**Tasks**:
1. ✅ Display current release
   - Release timestamp
   - Git commit hash
   - Deploy time

2. ✅ Display process status
   - Running processes
   - PID, uptime
   - Memory usage

3. ✅ Display Caddy status
   - Caddy running status
   - Current routing
   - Certificate status

4. ✅ Display available releases
   - List all releases
   - Sizes

5. ✅ Health check status
   - Current health
   - Response time

**Deliverable**: Working `bsdeploy status` command

### Phase 12: Error Handling & Logging (2-3 days)

**Goal**: Robust error handling and useful logging

**Tasks**:
1. ✅ Define error types
   - SSH errors
   - Configuration errors
   - Deployment errors
   - Use thiserror

2. ✅ Implement error context
   - Add context to errors
   - Use anyhow

3. ✅ Implement structured logging
   - Use tracing
   - Log levels (debug, info, warn, error)
   - Optional verbose mode

4. ✅ Add progress indicators
   - Show deployment progress
   - Spinners for long operations
   - Use indicatif

5. ✅ Implement cleanup on failure
   - Rollback partial deployments
   - Clean up temporary files
   - Clear error messages

**Deliverable**: Good error messages and logging

### Phase 13: Documentation (2-3 days)

**Goal**: Comprehensive documentation

**Tasks**:
1. ✅ Write README.md
   - Installation instructions
   - Quick start guide
   - Basic usage examples

2. ✅ Write configuration guide
   - All config options documented
   - Examples for common scenarios

3. ✅ Write deployment guide
   - Step-by-step setup
   - Deployment workflow
   - Troubleshooting

4. ✅ Add inline code documentation
   - Rustdoc comments
   - Module documentation
   - Public API documentation

5. ✅ Create example configs
   - Rails + SQLite example
   - Rails + PostgreSQL example (future)

6. ✅ Write contributing guide
   - Development setup
   - Testing instructions
   - Code style

**Deliverable**: Complete documentation

### Phase 14: Testing (3-4 days)

**Goal**: Comprehensive test coverage

**Tasks**:
1. ✅ Unit tests
   - Config parsing
   - Command builders
   - Template rendering

2. ✅ Integration tests
   - Full setup flow
   - Full deploy flow
   - Rollback flow

3. ✅ Add test fixtures
   - Sample configs
   - Mock SSH responses

4. ✅ Add CI/CD
   - GitHub Actions
   - Run tests on push
   - Check formatting
   - Run clippy

5. ✅ Manual testing
   - Deploy to real FreeBSD server
   - Test all commands
   - Verify zero-downtime

**Deliverable**: High test coverage, passing CI

### Phase 15: Polish & Release (1-2 days)

**Goal**: Prepare for v0.1.0 release

**Tasks**:
1. ✅ Version number
   - Set to 0.1.0
   - Update Cargo.toml

2. ✅ Changelog
   - Document all features
   - List known limitations

3. ✅ Release build
   - Optimize binary size
   - Strip debug symbols

4. ✅ Publish crate (optional)
   - Publish to crates.io
   - Or provide binary releases

5. ✅ Announcement
   - Blog post
   - Share with community

**Deliverable**: v0.1.0 release

## Future Enhancements (Post v1)

### Phase 16+: FreeBSD Jails

- Deploy each app in its own jail
- Jail templates and management
- Network isolation
- Resource limits (CPU, memory)

### Phase 17+: Multi-Server Support

- Deploy to multiple servers
- Server roles (web, worker, etc.)
- Parallel deployment
- Load balancing across servers

### Phase 18+: Database Management

- PostgreSQL support
- Database backup/restore
- Migration rollback
- Database connection pooling

### Phase 19+: Monitoring & Metrics

- Application metrics
- Error tracking integration
- Log aggregation
- Performance monitoring

### Phase 20+: Advanced Features

- Blue-green deployments
- Canary deployments
- A/B testing support
- Feature flags
- Auto-scaling

## Success Criteria

### Minimum Viable Product (v0.1.0)

- ✅ Deploy Rails + SQLite app to FreeBSD
- ✅ Zero-downtime deployments
- ✅ Automatic HTTPS via Caddy
- ✅ Rollback to previous version
- ✅ Simple TOML configuration
- ✅ Good error messages
- ✅ Basic documentation

### Quality Metrics

- Code compiles without warnings
- Passes all tests
- Deploys successfully to FreeBSD 14.x
- Zero-downtime verified (no dropped requests)
- Rollback works reliably
- Documentation is clear and accurate

## Timeline Estimate

**Total: 8-12 weeks** (part-time development)

- Phases 0-7: Foundation (4-6 weeks)
- Phases 8-11: Core Commands (4-5 weeks)
- Phases 12-15: Polish & Release (2-3 weeks)

## Open Questions

1. **Process Manager**: Use rc.d directly or consider supervisor/daemon tools?
   - **Decision**: Start with rc.d, evaluate later

2. **Puma Configuration**: How to manage Puma workers/threads?
   - **Decision**: Generate puma.rb from config

3. **Log Management**: Where to store logs long-term?
   - **Decision**: Shared log directory, rotation via newsyslog

4. **Database Backups**: Should the tool handle backups?
   - **Decision**: Not in v1, add later

5. **Secrets Management**: Support for external secret stores (Vault, etc.)?
   - **Decision**: Not in v1, simple file-based secrets

6. **Binary Distribution**: Publish to crates.io or provide binaries?
   - **Decision**: Both - source on crates.io, binaries on GitHub

## Conclusion

This design provides a solid foundation for a FreeBSD-native deployment tool in Rust. The incremental implementation plan ensures steady progress with testable milestones at each phase. The tool will provide zero-downtime deployments for Rails applications on FreeBSD with a simple, opinionated approach that leverages FreeBSD's native capabilities.

The design is intentionally minimal for v1, with clear paths for enhancement in future versions (jails, multi-server, advanced deployment strategies).
