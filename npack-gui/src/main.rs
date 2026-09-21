//! npack-gui: a small reference GUI exercising npackd's JSON-RPC service
//! API (Phase 5 of the roadmap), proving `npackd` is frontend-independent
//! before integrating with an existing desktop store. It talks to npackd
//! purely over its documented Unix-socket protocol (see `client.rs`) --
//! nothing here depends on the `npack` crate's internals.

mod app;
mod client;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "npack-gui",
        options,
        Box::new(|_cc| Ok(Box::new(app::NpackGuiApp::new()))),
    )
}
