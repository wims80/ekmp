#[cfg(feature = "gui")]
mod app;
mod cli;
mod clock;
mod core;
mod integrations;
mod killmail;
mod models;
mod persistence;

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(cli::run())
}
