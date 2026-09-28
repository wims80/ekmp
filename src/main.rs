#[cfg(feature = "gui")]
mod app;
mod cli;
mod core;
mod integrations;
mod killmail;
mod models;
mod persistence;

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(cli::run())
}

#[cfg(feature = "gui")]
fn launch_gui(
    scenario: Option<String>,
    dev_state: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let inspection = std::env::var("EGUI_INSPECTION")
        .is_ok_and(|value| !value.is_empty() && value != "0" && value != "false");
    if inspection && scenario.is_none() {
        return Err(
            "EGUI_INSPECTION may only be enabled together with a simulation scenario".into(),
        );
    }
    let app = match scenario {
        None => app::App::new(),
        Some(name) => {
            #[cfg(feature = "dev-tools")]
            {
                let loaded = integrations::simulation::load(&name)?;
                app::App::simulated(
                    loaded.store,
                    std::sync::Arc::new(loaded.backend),
                    loaded.name,
                    dev_state,
                    false,
                )
            }
            #[cfg(not(feature = "dev-tools"))]
            {
                let _ = dev_state;
                return Err(format!("scenario {name:?} requires --features dev-tools"));
            }
        }
    };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png"))
        .map_err(|_| "could not decode application icon")?;
    eframe::run_native(
        "EVE Killmail Publisher",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_app_id("ekmp")
                .with_inner_size([1180.0, 760.0])
                .with_min_inner_size([900.0, 620.0])
                .with_icon(icon),
            ..Default::default()
        },
        Box::new(move |_| Ok(Box::new(app))),
    )
    .map_err(|_| "could not open desktop interface".into())
}
