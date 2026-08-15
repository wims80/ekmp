mod app;
mod auth;
mod esi;
mod killmail;
mod models;
mod secrets;
mod storage;
mod zkill;

fn app_icon() -> eframe::Result<eframe::egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon.png"))
        .map_err(|error| eframe::Error::AppCreation(Box::new(error)))
}

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id("ekmp")
            .with_icon(app_icon()?),
        ..Default::default()
    };

    eframe::run_native(
        "EVE Killmail Publisher",
        native_options,
        Box::new(|_| Ok(Box::new(app::App::new()))),
    )
}

#[cfg(test)]
mod tests {
    use super::app_icon;

    #[test]
    fn embedded_app_icon_is_256_pixels_square() {
        let icon = app_icon().expect("embedded app icon should decode");

        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
