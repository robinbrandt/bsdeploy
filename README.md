# bsdeploy

A deployment tool to FreeBSD servers using jails. Inspired by [Kamal](https://kamal-deploy.org/), but instead of Linux and Docker, it uses FreeBSD and jails. Supports only a small subset that I need for deploying side projects and some open source software on a root server.

## Features

- **Zero-downtime deployments** via blue/green jail switching
- **ZFS support** with snapshots and clones for fast jail creation
- **Caddy integration** for automatic HTTPS and reverse proxying
- **mise support** for managing language runtimes (Ruby, Node, Python, etc.)
- **Environment management** with clear and secret variables
- **PF firewall configuration** for jail NAT (outbound traffic)
- **Boot persistence** via rc.d service (jails automatically restart after reboot)

## Current not supported

A non-exhaustive list:

- **Multiple roles** for the application servers (worker servers)
- **Accessories** (e.g. provisioning a database server)
- **Parallel deployments to multiple hosts**
- ...

## Installation

```bash
cargo install --path .
```

## Quick Start

1. Initialize a configuration file:

```bash
bsdeploy init
```

2. Edit `config/bsdeploy.yml`:

```yaml
service: myapp

hosts:
  - bsd.example.com

doas: true

# User to run the application as (created inside jails)
user: myapp

proxy:
  hostname: myapp.example.com
  port: 3000

# Packages installed inside jails
packages:
  - curl

# Language runtimes installed inside jails via mise
mise:
  ruby: 3.4.7

env:
  clear:
    - PORT: "3000"
    - RAILS_ENV: production
  secret:
    - SECRET_KEY_BASE

# Persistent directories mounted into jails (survives deploys)
data_directories:
  - /var/db/myapp/storage:/app/storage

before_start:
  - bundle install
  - bin/rails db:migrate

start:
  - bin/rails server
```

3. Set up the remote host:

```bash
bsdeploy setup
```

4. Deploy:

```bash
bsdeploy deploy
```

## Commands

| Command | Description |
|---------|-------------|
| `bsdeploy init` | Create a new configuration file |
| `bsdeploy setup` | Prepare remote hosts (install Caddy, configure PF, etc.) |
| `bsdeploy deploy` | Build and deploy the application |
| `bsdeploy destroy` | Remove all resources for the service |

### Setup Options

| Option | Description |
|--------|-------------|
| `--force-pf` | Append bsdeploy PF rules to an existing `/etc/pf.conf` |

By default, `bsdeploy setup` will fail if the host already has a custom `/etc/pf.conf` to avoid overwriting existing firewall rules. Use `--force-pf` to prepend the NAT rules required for jail traffic.

## How It Works

1. **Setup** installs host-level packages (Caddy, rsync, git, bash), creates directories, configures the reverse proxy, and sets up PF for jail NAT on each host
2. **Deploy**:
   - Builds a reusable jail image containing your packages and mise tools
   - Creates a new jail from the image
   - Syncs your application code via rsync
   - Runs `before_start` commands inside the jail (migrations, asset compilation, etc.)
   - Starts your application as a daemon inside the jail
   - Switches Caddy to route traffic to the new jail
   - Gracefully stops old jails
3. Old jails are kept for rollback and eventually pruned

## Boot Persistence

Deployed jails automatically restart after a system reboot. During `bsdeploy setup`, an rc.d service is installed and enabled. Each deploy writes metadata to the jail that allows the service to reconstruct the jail environment on boot.

**Service commands** (run on the remote host):

| Command | Description |
|---------|-------------|
| `service bsdeploy start` | Start all active jails |
| `service bsdeploy stop` | Stop all active jails |
| `service bsdeploy status` | Show status of all active jails |
| `service bsdeploy restart` | Restart all active jails |

The service handles:
- Creating the loopback interface (`lo1`) and IP aliases
- Mounting base system, image, and data directories
- Starting the jail and application processes
- Proper shutdown and unmounting on stop

## Configuration Reference

| Option | Description |
|--------|-------------|
| `service` | Name of your application (used for jail naming, directories) |
| `hosts` | List of FreeBSD hosts to deploy to |
| `doas` | Use doas for privilege escalation (default: false) |
| `user` | Unix user created inside jails to run the application |
| `packages` | FreeBSD packages installed inside jails |
| `mise` | Language runtimes installed inside jails via mise |
| `proxy` | Caddy reverse proxy configuration (see below) |
| `env.clear` | Environment variables (stored in config) |
| `env.secret` | Environment variables (read from local shell at deploy time) |
| `before_start` | Commands run inside jail before starting (e.g., migrations) |
| `start` | Commands to start your application (run as daemons) |
| `data_directories` | Persistent directories mounted into jails |
| `jail.ip_range` | IP range for jails (default: `10.0.0.0/24`, used for PF NAT) |

### Proxy Configuration

The `proxy` section configures Caddy as a reverse proxy with TLS:

```yaml
proxy:
  hostname: myapp.example.com
  port: 3000
```

**TLS Options:**

| Mode | Configuration | Description |
|------|---------------|-------------|
| ACME (default) | `tls: true` or omitted | Caddy automatically obtains Let's Encrypt certificates |
| Disabled | `tls: false` | Plain HTTP, no TLS |
| Custom SSL | `ssl: { ... }` | Use your own certificates |

**Custom SSL Certificates:**

When Let's Encrypt is not suitable (e.g., internal domains, specific CA requirements), you can provide your own certificates:

```yaml
proxy:
  hostname: myapp.example.com
  port: 3000
  ssl:
    certificate_pem: SSL_CERTIFICATE_PEM
    private_key_pem: SSL_PRIVATE_KEY_PEM
```

The `certificate_pem` and `private_key_pem` values are environment variable names. Set them before deploying:

```bash
export SSL_CERTIFICATE_PEM="$(cat /path/to/cert.pem)"
export SSL_PRIVATE_KEY_PEM="$(cat /path/to/key.pem)"
bsdeploy deploy
```

Certificates are written to `/usr/local/etc/caddy/certs/` on the remote host with secure permissions.

## License

MIT
