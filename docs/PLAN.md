# Implementation Plan

## Phase 1: Project Initialization & Configuration
- [x] Initialize Rust project (`cargo init`).
- [x] Add dependencies: `clap`, `serde`, `serde_yaml`, `anyhow`, `log`, `env_logger`.
- [x] Define `Config` structs in `src/config.rs`.
- [x] Implement `bsdeploy setup` and `bsdeploy deploy` stubs.
- [x] Implement configuration loading logic.

## Phase 2: SSH Command Wrapper
- [x] Create a `Remote` struct/module to handle SSH execution.
- [x] Implement `run(host, command)` using `std::process::Command` calling `ssh`.
- [x] Test connectivity logic.

## Phase 3: The `setup` Command
- [x] Implement package installation logic (`pkg install -y ...`).
- [x] Implement Caddy installation check.
- [x] Implement Environment variable handling:
    - [x] Read local env vars for `secret` keys.
    - [x] Generate env file content.
    - [x] Write env file to remote (via `ssh` + `cat` or `scp`).

## Phase 4: The `deploy` Command - Shipping
- [x] Implement `rsync` wrapper to sync current directory to remote.
- [x] Handle excludes (hardcoded defaults like `.git` + user config if needed).

## Phase 5: The `deploy` Command - Execution
- [x] Implement execution of `before_start` commands.
- [x] Implement execution of `start` command.
    - [x] Use `daemon` utility on FreeBSD to background the process and manage PID files.

## Phase 6: Polish
- [x] Add logging/output styling (maybe `indicatif` for spinners).
- [x] Error handling improvements.
