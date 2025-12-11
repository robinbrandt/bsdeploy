# Caddy Proxy Implementation Plan

## 1. Configuration
- Update `Config` struct in `src/config.rs` to include an optional `proxy` section.
    - `hostname`: String (e.g., "myapp.com")
    - `port`: u16 (Internal app port, e.g., 8080)

## 2. Caddyfile Generation
- In `src/main.rs` (or a new module), implement logic to generate a simple `Caddyfile`.
- Format:
  ```
  hostname {
      reverse_proxy :port
  }
  ```

## 3. Deployment (Setup Command)
- In `bsdeploy setup`:
    - Check if `proxy` config exists.
    - If so, generate the `Caddyfile` content.
    - Write it to `/usr/local/etc/caddy/Caddyfile` (or `/usr/local/etc/caddy/Caddyfile.d/<service>` if using imports, but we'll stick to a single file or a conf.d approach if simple).
    - **Decision**: To avoid overwriting other sites, we should probably use a `conf.d` approach.
        - Ensure main `Caddyfile` imports `/usr/local/etc/caddy/conf.d/*`.
        - Write our config to `/usr/local/etc/caddy/conf.d/<service>.caddy`.
    - Reload Caddy: `service caddy reload` or `service caddy start` if not running.

## 4. Enable Caddy
- Ensure `sysrc caddy_enable=YES` is run.
