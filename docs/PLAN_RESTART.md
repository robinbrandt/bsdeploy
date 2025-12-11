# Restart Command Implementation Plan

## 1. CLI Update
- [x] Add `Restart` variant to `Commands` enum in `src/main.rs`.

## 2. Logic Implementation
- [x] The `restart` command should mimic the restart logic in `deploy` but without syncing code.
- Steps:
    1. Read PID file path: `/var/run/bsdeploy_{service}.pid`.
    2. Read Log file path: `/var/log/bsdeploy_{service}.log`.
    3. Kill existing process: `pkill -F {pid_file}`.
    4. Start new process: Execute `start` commands using `daemon` (same logic as `deploy`).

## 3. Reuse
- [x] Refactor the start logic into a shared helper function in `src/main.rs` (or `src/deploy.rs` if we split it) to avoid duplication between `deploy` and `restart`.