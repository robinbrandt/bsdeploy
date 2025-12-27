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
user: myapp

proxy:
  hostname: myapp.example.com
  port: 3000

packages:
  - curl

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
| `bsdeploy setup` | Prepare remote hosts (install packages, create user, configure Caddy) |
| `bsdeploy deploy` | Build and deploy the application |
| `bsdeploy destroy` | Remove all resources for the service |

## How It Works

1. **Setup** installs required packages, creates the application user, and configures Caddy on each host
2. **Deploy** creates a new jail with your application, syncs code via rsync, runs setup commands, then switches traffic to the new jail
3. Old jails are kept for rollback (configurable) and eventually pruned

## License

MIT
