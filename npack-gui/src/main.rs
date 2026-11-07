//! npack-gui: a small reference GUI exercising npackd's JSON-RPC service
//! API (Phase 5 of the roadmap), proving `npackd` is frontend-independent
//! before integrating with an existing desktop store. It talks to npackd
//! purely over its documented Unix-socket protocol (see `client.rs`) --
//! nothing here depends on the `npack` crate's internals.

mod app;
mod client;
mod friendly;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("npack")
            // Matches StartupWMClass in the .desktop file, so the desktop can
            // tie this window to its launcher entry and icon.
            .with_app_id("io.npack.Gui")
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "npack",
        options,
        Box::new(|cc| {
            // PNG and SVG decoding for the store icons npackd delivers.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::NpackGuiApp::new(cc.egui_ctx.clone())))
        }),
    )
}
