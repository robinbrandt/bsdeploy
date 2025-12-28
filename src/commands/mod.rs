mod deploy;
mod destroy;
mod init;
mod setup;
mod status;

pub use deploy::run as deploy;
pub use destroy::run as destroy;
pub use init::run as init;
pub use setup::run as setup;
pub use status::run as status;

/// Build a command with optional doas prefix.
pub fn maybe_doas(cmd: &str, doas: bool) -> String {
    if doas {
        format!("doas {}", cmd)
    } else {
        cmd.to_string()
    }
}
