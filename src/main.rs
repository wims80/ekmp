#[cfg(feature = "gui")]
mod app;
mod cli;
mod clock;
mod core;
mod integrations;
mod killmail;
mod models;
mod persistence;

// The offline simulator and egui inspection are development tools. Keep them out of
// optimized builds so a release binary can never contain them.
#[cfg(all(feature = "dev-tools", not(debug_assertions)))]
compile_error!("the dev-tools feature is only for debug builds; build without --release");

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(cli::run())
}
