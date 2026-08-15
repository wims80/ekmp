mod app;
mod auth;
mod esi;
mod killmail;
mod models;
mod storage;
mod zkill;

fn main() -> eframe::Result {
    eframe::run_native(
        "akmp",
        eframe::NativeOptions::default(),
        Box::new(|_| Ok(Box::new(app::App::new()))),
    )
}
