# bsdeploy

A deployment tool for FreeBSD using jails. Inspired by [Kamal](https://kamal-deploy.org/), but instead of Linux and Docker, it uses FreeBSD and jails.

## Features

- **Zero-downtime deployments** via blue/green jail switching
- **ZFS support** with snapshots and clones for fast jail creation
- **Caddy integration** for automatic HTTPS and reverse proxying
- **mise support** for managing language runtimes (Ruby, Node, Python, etc.)
- **Environment management** with clear and secret variables

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
| `bsdeploy setup` | Prepare remote hosts (install Caddy, configure directories) |
| `bsdeploy deploy` | Build and deploy the application |
| `bsdeploy destroy` | Remove all resources for the service |

## How It Works

1. **Setup** installs host-level packages (Caddy, rsync, git, bash), creates directories, and configures the reverse proxy on each host
2. **Deploy**:
   - Builds a reusable jail image containing your packages and mise tools
   - Creates a new jail from the image
   - Syncs your application code via rsync
   - Runs `before_start` commands inside the jail (migrations, asset compilation, etc.)
   - Starts your application as a daemon inside the jail
   - Switches Caddy to route traffic to the new jail
   - Gracefully stops old jails
3. Old jails are kept for rollback and eventually pruned

## Configuration Reference

| Option | Description |
|--------|-------------|
| `service` | Name of your application (used for jail naming, directories) |
| `hosts` | List of FreeBSD hosts to deploy to |
| `doas` | Use doas for privilege escalation (default: false) |
| `user` | Unix user created inside jails to run the application |
| `packages` | FreeBSD packages installed inside jails |
| `mise` | Language runtimes installed inside jails via mise |
| `proxy` | Caddy reverse proxy configuration |
| `env.clear` | Environment variables (stored in config) |
| `env.secret` | Environment variables (read from local shell at deploy time) |
| `before_start` | Commands run inside jail before starting (e.g., migrations) |
| `start` | Commands to start your application (run as daemons) |
| `data_directories` | Persistent directories mounted into jails |

## License

MIT
