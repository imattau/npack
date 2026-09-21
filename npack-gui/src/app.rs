use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use serde_json::Value;

use crate::client::DaemonClient;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Installed,
    Search,
    Updates,
}

/// One background result delivered back into the UI thread. Every network
/// call (`DaemonClient::call`) happens on a spawned thread; the app's
/// `logic()` drains this channel each frame with `try_recv` so a slow
/// relay or Blossom fetch never freezes the window.
enum Event {
    Installed(Result<Vec<Value>, String>),
    Search(Result<Vec<Value>, String>),
    Updates(Result<Vec<Value>, String>),
    Details(Result<Value, String>),
    /// `Install`/`Update` were started with `"async": true`; this carries
    /// the transaction id so the app can poll `GetTransaction`.
    TransactionStarted {
        action: &'static str,
        id: u64,
    },
    TransactionProgress(Value),
    /// A `Remove`, or a completed/failed/cancelled transaction.
    ActionFinished {
        action: &'static str,
        result: Result<Value, String>,
    },
}

pub struct NpackGuiApp {
    client: Arc<DaemonClient>,
    socket_path_text: String,
    tab: Tab,

    installed: Vec<Value>,
    search_query: String,
    search_relays: String,
    search_rebuild_catalogue: bool,
    search_results: Vec<Value>,
    updates: Vec<Value>,

    details: Option<Value>,
    status: String,
    active_transaction: Option<(&'static str, u64)>,

    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
}

impl NpackGuiApp {
    pub fn new() -> Self {
        let socket_path = DaemonClient::default_socket_path();
        let (tx, rx) = mpsc::channel();
        Self {
            socket_path_text: socket_path.display().to_string(),
            client: Arc::new(DaemonClient::new(socket_path)),
            tab: Tab::Installed,
            installed: Vec::new(),
            search_query: String::new(),
            search_relays: String::new(),
            search_rebuild_catalogue: false,
            search_results: Vec::new(),
            updates: Vec::new(),
            details: None,
            status: "Not connected yet -- pick a tab to load data.".to_owned(),
            active_transaction: None,
            tx,
            rx,
        }
    }

    fn spawn<F>(&self, work: F)
    where
        F: FnOnce(&DaemonClient) -> Event + Send + 'static,
    {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let event = work(&client);
            let _ = tx.send(event);
        });
    }

    fn refresh_installed(&self) {
        self.spawn(|client| {
            let result = client
                .call("ListInstalled", serde_json::json!({}))
                .map(|value| value.as_array().cloned().unwrap_or_default())
                .map_err(|error| error.to_string());
            Event::Installed(result)
        });
    }

    fn run_search(&self) {
        let query = self.search_query.clone();
        let relays: Vec<String> = self
            .search_relays
            .split(',')
            .map(str::trim)
            .filter(|relay| !relay.is_empty())
            .map(str::to_owned)
            .collect();
        let rebuild = self.search_rebuild_catalogue;
        self.spawn(move |client| {
            let params = serde_json::json!({
                "query": query,
                "relay": relays,
                "refresh": rebuild,
            });
            let result = client
                .call("Search", params)
                .map(|value| value.as_array().cloned().unwrap_or_default())
                .map_err(|error| error.to_string());
            Event::Search(result)
        });
    }

    fn refresh_updates(&self) {
        self.spawn(|client| {
            let result = client
                .call("CheckUpdates", serde_json::json!({}))
                .map(|value| value.as_array().cloned().unwrap_or_default())
                .map_err(|error| error.to_string());
            Event::Updates(result)
        });
    }

    fn show_details(&self, package: &str) {
        let package = package.to_owned();
        self.spawn(move |client| {
            let result = client
                .call("GetPackage", serde_json::json!({ "package": package }))
                .map_err(|error| error.to_string());
            Event::Details(result)
        });
    }

    fn install(&mut self, package: &str) {
        let package = package.to_owned();
        self.status = format!("Starting install of {package}...");
        self.spawn(move |client| {
            let params = serde_json::json!({ "package": package, "async": true });
            match client.call("Install", params) {
                Ok(value) => match value.get("transaction_id").and_then(Value::as_u64) {
                    Some(id) => Event::TransactionStarted {
                        action: "install",
                        id,
                    },
                    None => Event::ActionFinished {
                        action: "install",
                        result: Ok(value),
                    },
                },
                Err(error) => Event::ActionFinished {
                    action: "install",
                    result: Err(error.to_string()),
                },
            }
        });
    }

    fn remove(&mut self, package: &str) {
        let package = package.to_owned();
        self.status = format!("Removing {package}...");
        self.spawn(move |client| {
            let result = client
                .call("Remove", serde_json::json!({ "package": package }))
                .map_err(|error| error.to_string());
            Event::ActionFinished {
                action: "remove",
                result,
            }
        });
    }

    /// Polls `GetTransaction` until it leaves the `running` state, sending
    /// a `TransactionProgress` event on every poll and a final
    /// `ActionFinished` -- unlike `spawn`, which only ever delivers one
    /// event, this thread sends many over the lifetime of the transaction.
    fn poll_transaction(&self, action: &'static str, id: u64) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            loop {
                match client.call(
                    "GetTransaction",
                    serde_json::json!({ "transaction_id": id }),
                ) {
                    Ok(status) => {
                        if status.get("status").and_then(Value::as_str) != Some("running") {
                            let _ = tx.send(Event::ActionFinished {
                                action,
                                result: Ok(status),
                            });
                            return;
                        }
                        let _ = tx.send(Event::TransactionProgress(status));
                        thread::sleep(Duration::from_millis(200));
                    }
                    Err(error) => {
                        let _ = tx.send(Event::ActionFinished {
                            action,
                            result: Err(error.to_string()),
                        });
                        return;
                    }
                }
            }
        });
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Installed(Ok(items)) => {
                    self.status = format!("{} package(s) installed.", items.len());
                    self.installed = items;
                }
                Event::Installed(Err(error)) => {
                    self.status = format!("ListInstalled failed: {error}")
                }
                Event::Search(Ok(items)) => {
                    self.status = format!("{} result(s).", items.len());
                    self.search_results = items;
                }
                Event::Search(Err(error)) => self.status = format!("Search failed: {error}"),
                Event::Updates(Ok(items)) => {
                    self.status = format!("Checked {} installed package(s).", items.len());
                    self.updates = items;
                }
                Event::Updates(Err(error)) => self.status = format!("CheckUpdates failed: {error}"),
                Event::Details(Ok(value)) => self.details = Some(value),
                Event::Details(Err(error)) => self.status = format!("GetPackage failed: {error}"),
                Event::TransactionStarted { action, id } => {
                    self.active_transaction = Some((action, id));
                    self.status =
                        format!("{action} started (transaction {id}), watching progress...");
                    self.poll_transaction(action, id);
                }
                Event::TransactionProgress(status) => {
                    let stage = status
                        .get("progress")
                        .and_then(|progress| progress.get("stage"))
                        .and_then(Value::as_str)
                        .unwrap_or("working");
                    let package = status
                        .get("progress")
                        .and_then(|progress| progress.get("package"))
                        .and_then(Value::as_str);
                    self.status = match package {
                        Some(package) => format!("{stage}: {package}"),
                        None => stage.to_owned(),
                    };
                }
                Event::ActionFinished { action, result } => {
                    self.active_transaction = None;
                    match result {
                        Ok(value) => {
                            self.status = format!("{action} finished: {value}");
                            self.refresh_installed();
                        }
                        Err(error) => self.status = format!("{action} failed: {error}"),
                    }
                }
            }
        }
    }
}

impl Default for NpackGuiApp {
    fn default() -> Self {
        Self::new()
    }
}

fn value_str(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned()
}

fn package_reference(value: &Value) -> String {
    format!(
        "{}/{}",
        value_str(value, "publisher"),
        value_str(value, "name")
    )
}

impl eframe::App for NpackGuiApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        // Redraw regularly while a background call or transaction is in
        // flight, so polling results and progress show up promptly.
        if self.active_transaction.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("connection").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("npackd socket:");
                ui.text_edit_singleline(&mut self.socket_path_text);
                if ui.button("Reconnect").clicked() {
                    self.client =
                        Arc::new(DaemonClient::new(PathBuf::from(&self.socket_path_text)));
                    self.status = "Socket path updated.".to_owned();
                }
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });

        egui::Panel::right("details").show(ui, |ui| {
            ui.heading("Details");
            match &self.details {
                Some(value) => {
                    ui.label(serde_json::to_string_pretty(value).unwrap_or_default());
                }
                None => {
                    ui.label("Select a package and click Details.");
                }
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Installed, "Installed");
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.selectable_value(&mut self.tab, Tab::Updates, "Updates");
            });
            ui.separator();

            match self.tab {
                Tab::Installed => self.show_installed_tab(ui),
                Tab::Search => self.show_search_tab(ui),
                Tab::Updates => self.show_updates_tab(ui),
            }
        });
    }
}

impl NpackGuiApp {
    fn show_installed_tab(&mut self, ui: &mut egui::Ui) {
        if ui.button("Refresh installed list").clicked() {
            self.refresh_installed();
        }
        ui.separator();
        let mut to_remove = None;
        let mut to_show = None;
        for item in &self.installed {
            let reference = package_reference(item);
            ui.horizontal(|ui| {
                ui.label(format!("{reference} {}", value_str(item, "version")));
                if ui.button("Details").clicked() {
                    to_show = Some(reference.clone());
                }
                if ui.button("Remove").clicked() {
                    to_remove = Some(reference.clone());
                }
            });
        }
        if let Some(reference) = to_show {
            self.show_details(&reference);
        }
        if let Some(reference) = to_remove {
            self.remove(&reference);
        }
    }

    fn show_search_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Query:");
            ui.text_edit_singleline(&mut self.search_query);
        });
        ui.horizontal(|ui| {
            ui.label("Relays (comma-separated):");
            ui.text_edit_singleline(&mut self.search_relays);
        });
        ui.checkbox(
            &mut self.search_rebuild_catalogue,
            "Rebuild local catalogue from relays first (npack refresh)",
        );
        if ui.button("Search").clicked() {
            self.run_search();
        }
        ui.separator();
        let mut to_install = None;
        let mut to_show = None;
        for item in &self.search_results {
            let reference = package_reference(item);
            ui.horizontal(|ui| {
                ui.label(format!("{reference} {}", value_str(item, "version")));
                if ui.button("Details").clicked() {
                    to_show = Some(reference.clone());
                }
                if ui.button("Install").clicked() {
                    to_install = Some(reference.clone());
                }
            });
        }
        if let Some(reference) = to_show {
            self.show_details(&reference);
        }
        if let Some(reference) = to_install {
            self.install(&reference);
        }
    }

    fn show_updates_tab(&mut self, ui: &mut egui::Ui) {
        if ui.button("Check for updates").clicked() {
            self.refresh_updates();
        }
        ui.separator();
        for item in &self.updates {
            let reference = value_str(item, "reference");
            let current = value_str(item, "current_version");
            let available = item
                .get("available_version")
                .and_then(Value::as_str)
                .map(str::to_owned);
            match available {
                Some(available) => {
                    ui.label(format!("{reference}: {current} -> {available} available"));
                }
                None => {
                    ui.label(format!("{reference}: {current} up to date"));
                }
            }
        }
    }
}
