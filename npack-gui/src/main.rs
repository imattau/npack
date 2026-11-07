//! npack-gui: a small reference GUI exercising npackd's JSON-RPC service
//! API (Phase 5 of the roadmap), proving `npackd` is frontend-independent
//! before integrating with an existing desktop store. It talks to npackd
//! purely over its documented Unix-socket protocol (see `client.rs`) --
//! nothing here depends on the `npack` crate's internals.

mod app;
mod client;
mod friendly;
mod launch;
mod remember;
mod style;

/// The first `.npk` named on the command line (`npack-gui app.npk`), as an
/// absolute path, so a file manager can hand a package to the app.
fn package_from_arguments() -> Option<std::path::PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .find(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("npk"))
        })
        .and_then(|path| std::path::absolute(path).ok())
}

fn main() -> eframe::Result<()> {
    let remembered = remember::load();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("npack")
            // Matches StartupWMClass in the .desktop file, so the desktop can
            // tie this window to its launcher entry and icon.
            .with_app_id("io.npack.Gui")
            .with_inner_size(remembered.window_size())
            .with_min_inner_size(remember::MIN_SIZE),
        ..Default::default()
    };
    eframe::run_native(
        "npack",
        options,
        Box::new(|cc| {
            // PNG and SVG decoding for the store icons npackd delivers.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            style::install(&cc.egui_ctx);
            Ok(Box::new(app::NpackGuiApp::new(
                cc.egui_ctx.clone(),
                package_from_arguments(),
            )))
        }),
    )
}
