use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub fn print_step(msg: &str) {
    println!("{} {}", "::".blue().bold(), msg.bold());
}

pub fn print_success(msg: &str) {
    println!("{} {}", "✔".green().bold(), msg.green());
}

pub fn print_error(msg: &str) {
    eprintln!("{} {}", "✖".red().bold(), msg.red());
}

pub fn create_spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
            .template("{spinner:.blue} {msg}")
            .unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}
