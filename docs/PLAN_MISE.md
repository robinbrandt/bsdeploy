# Mise Support Implementation Plan

## 1. Configuration
- [x] Update `Config` struct in `src/config.rs`.
- [x] Add `mise` field: `Option<HashMap<String, String>>`.
  - Key: Tool name (e.g., "node", "ruby").
  - Value: Version (e.g., "20", "3.2").

## 2. Setup Logic (`bsdeploy setup`)
- [x] **Install Mise**: `pkg install -y mise` (or curl/sh script if pkg is outdated, but pkg is preferred on BSD).
- [x] **Configure Shell**: Ensure `mise activate` is added to the environment script (`env` file) we generate.
    - We source `/usr/local/etc/bsdeploy/<service>/env`.
    - We added `eval "$(mise activate sh)"` to it.
- [x] **Install Tools**:
    - Iterate over the `mise` map in config.
    - Run `mise use -g tool@version` on the remote host.
    - Command: `mise use --global <tool>@<version>`

## 3. Environment Persistence
- [x] Ensure `mise` bin path is in PATH or `mise activate` is run before `start` commands.
- Our `env` file is sourced before `start` commands. So adding `eval "$(mise activate sh)"` to the `env` file is the correct approach.