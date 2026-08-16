mod app;
mod integrations;
mod killmail;
mod models;
mod persistence;

use std::{path::PathBuf, process::ExitCode};

fn app_icon() -> eframe::Result<eframe::egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png"))
        .map_err(|error| eframe::Error::AppCreation(Box::new(error)))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ekmp: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let launch = LaunchOptions::parse(std::env::args_os().skip(1))?;
    if inspection_enabled() && launch.scenario.is_none() {
        return Err(
            "EGUI_INSPECTION may only be enabled together with a simulation scenario".into(),
        );
    }

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id("ekmp")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 620.0])
            .with_icon(app_icon().map_err(|error| error.to_string())?),
        ..Default::default()
    };

    let app = create_app(launch)?;

    eframe::run_native(
        "EVE Killmail Publisher",
        native_options,
        Box::new(move |_| Ok(Box::new(app))),
    )
    .map_err(|error| error.to_string())
}

#[derive(Debug)]
struct LaunchOptions {
    scenario: Option<String>,
    dev_state: Option<PathBuf>,
}

impl LaunchOptions {
    fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, String> {
        let mut args = args.into_iter();
        let mut scenario = None;
        let mut dev_state = None;
        while let Some(argument) = args.next() {
            match argument.to_str() {
                Some("--scenario") => {
                    let value = args
                        .next()
                        .ok_or("--scenario requires a name")?
                        .into_string()
                        .map_err(|_| "scenario names must be valid UTF-8")?;
                    scenario = Some(value);
                }
                Some("--dev-state") => {
                    dev_state = Some(PathBuf::from(
                        args.next().ok_or("--dev-state requires a path")?,
                    ));
                }
                Some(flag) => return Err(format!("unknown argument {flag:?}")),
                None => return Err("arguments must be valid UTF-8".into()),
            }
        }
        if dev_state.is_some() && scenario.is_none() {
            return Err("--dev-state may only be used with --scenario".into());
        }
        Ok(Self {
            scenario,
            dev_state,
        })
    }
}

fn create_app(launch: LaunchOptions) -> Result<app::App, String> {
    let Some(scenario_name) = launch.scenario else {
        return Ok(app::App::new());
    };

    #[cfg(feature = "dev-tools")]
    {
        use std::sync::Arc;

        let loaded = integrations::simulation::load(&scenario_name)?;
        let store = match launch.dev_state.as_deref() {
            Some(path) if path.is_file() => persistence::storage::load_from_path(path)?,
            _ => loaded.store,
        };
        Ok(app::App::simulated(
            store,
            Arc::new(loaded.backend),
            loaded.name,
            launch.dev_state,
            false,
        ))
    }

    #[cfg(not(feature = "dev-tools"))]
    {
        let _ = launch.dev_state;
        Err(format!(
            "scenario {scenario_name:?} requires a build with --features dev-tools"
        ))
    }
}

fn inspection_enabled() -> bool {
    std::env::var("EGUI_INSPECTION")
        .is_ok_and(|value| !value.is_empty() && value != "0" && value != "false")
}

#[cfg(test)]
mod tests {
    use super::{app_icon, LaunchOptions};

    #[test]
    fn embedded_app_icon_is_256_pixels_square() {
        let icon = app_icon().expect("embedded app icon should decode");

        assert_eq!((icon.width, icon.height), (256, 256));
    }

    #[test]
    fn development_state_requires_a_scenario() {
        let result = LaunchOptions::parse(["--dev-state".into(), "state.json".into()]);

        assert!(result.unwrap_err().contains("only be used with --scenario"));
    }
}
