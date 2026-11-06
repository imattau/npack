use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use eframe::egui;
use serde_json::Value;

use crate::{client::DaemonClient, friendly::UiError};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Installed,
    Search,
    Updates,
    Settings,
}

/// One background result delivered back into the UI thread. Every network
/// call (`DaemonClient::call`) happens on a spawned thread; the app's
/// `logic()` drains this channel each frame with `try_recv` so a slow
/// relay or Blossom fetch never freezes the window.
enum Event {
    Installed(Result<Vec<Value>, UiError>),
    Search(Result<Vec<Value>, UiError>),
    Updates(Result<Vec<Value>, UiError>),
    Details(Result<Value, UiError>),
    /// `GetConfig`: first-run flag, relays, download servers, trusted publishers.
    Config(Result<Value, UiError>),
    /// Extra package details for the "unknown publisher" dialog.
    InstallPreview(Result<Value, UiError>),
    /// Readable identities (verified NIP-05 names) from `GetPublisherProfiles`.
    Profiles(Result<Vec<Value>, UiError>),
    /// `ResolvePublisher`: who the typed npub / hex key / name@domain is.
    PublisherResolved(Result<Value, UiError>),
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
        result: Result<Value, UiError>,
    },
}

/// An install the user must confirm because its publisher isn't trusted yet.
struct PendingInstall {
    reference: String,
    publisher: String,
    name: String,
    version: String,
    /// `GetPackage` result (dependencies, runtime requirements), once loaded.
    preview: Option<Value>,
}

enum TrustChoice {
    TrustAndInstall,
    InstallOnce,
    Cancel,
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

    config: Option<Value>,
    welcome_dismissed: bool,
    error: Option<UiError>,
    show_error_details: bool,
    pending_install: Option<PendingInstall>,
    trust_input: String,
    /// The resolved publisher awaiting the user's "Trust" confirmation.
    trust_preview: Option<Value>,
    /// Publisher hex -> identity (names are display aids, never the identity).
    identities: HashMap<String, Value>,
    profiles_requested: HashSet<String>,
    relays_text: String,
    blossom_text: String,

    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
}

impl NpackGuiApp {
    pub fn new() -> Self {
        let socket_path = DaemonClient::default_socket_path();
        let (tx, rx) = mpsc::channel();
        let app = Self {
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
            config: None,
            welcome_dismissed: false,
            error: None,
            show_error_details: false,
            pending_install: None,
            trust_input: String::new(),
            trust_preview: None,
            identities: HashMap::new(),
            profiles_requested: HashSet::new(),
            relays_text: String::new(),
            blossom_text: String::new(),
            tx,
            rx,
        };
        app.refresh_config();
        app
    }

    /// Records a failed action: a short status plus a banner with plain-language
    /// advice (the daemon's raw message stays under "Details").
    fn fail(&mut self, action: &str, error: UiError) {
        self.status = format!("{action} didn't complete.");
        self.error = Some(error);
        self.show_error_details = false;
    }

    fn refresh_config(&self) {
        self.spawn(|client| {
            Event::Config(
                client
                    .call("GetConfig", serde_json::json!({}))
                    .map_err(|e| UiError::from_anyhow(&e)),
            )
        });
    }

    fn save_config(&self, relays: Option<Vec<String>>, blossom: Option<Vec<String>>) {
        self.spawn(move |client| {
            let mut params = serde_json::Map::new();
            if let Some(relays) = relays {
                params.insert("relays".into(), relays.into());
            }
            if let Some(blossom) = blossom {
                params.insert("blossom".into(), blossom.into());
            }
            Event::Config(
                client
                    .call("SaveConfig", Value::Object(params))
                    .map_err(|e| UiError::from_anyhow(&e)),
            )
        });
    }

    fn add_trusted(&self, publisher: String) {
        self.spawn(move |client| {
            match client.call(
                "AddTrustedPublisher",
                serde_json::json!({ "publisher": publisher }),
            ) {
                Ok(_) => Event::Config(
                    client
                        .call("GetConfig", serde_json::json!({}))
                        .map_err(|e| UiError::from_anyhow(&e)),
                ),
                Err(error) => Event::Config(Err(UiError::from_anyhow(&error))),
            }
        });
    }

    fn remove_trusted(&self, publisher: String) {
        self.spawn(move |client| {
            match client.call(
                "RemoveTrustedPublisher",
                serde_json::json!({ "publisher": publisher }),
            ) {
                Ok(_) => Event::Config(
                    client
                        .call("GetConfig", serde_json::json!({}))
                        .map_err(|e| UiError::from_anyhow(&e)),
                ),
                Err(error) => Event::Config(Err(UiError::from_anyhow(&error))),
            }
        });
    }

    /// Asks npackd for readable names for publishers we haven't looked up yet.
    /// Best effort and cached by the daemon; failures just leave npubs showing.
    fn request_profiles(&mut self, publishers: Vec<String>) {
        let wanted: Vec<String> = publishers
            .into_iter()
            .filter(|publisher| {
                !publisher.is_empty() && self.profiles_requested.insert(publisher.clone())
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.spawn(move |client| {
            Event::Profiles(
                client
                    .call(
                        "GetPublisherProfiles",
                        serde_json::json!({ "publishers": wanted }),
                    )
                    .map(|value| value.as_array().cloned().unwrap_or_default())
                    .map_err(|e| UiError::from_anyhow(&e)),
            )
        });
    }

    fn resolve_publisher(&self, query: String) {
        self.spawn(move |client| {
            Event::PublisherResolved(
                client
                    .call("ResolvePublisher", serde_json::json!({ "query": query }))
                    .map_err(|e| UiError::from_anyhow(&e)),
            )
        });
    }

    /// How to show a publisher: a verified name, a clearly-marked claim, or
    /// just the shortened npub.
    fn publisher_text(&self, hex: &str, npub: &str) -> String {
        describe_publisher(self.identities.get(hex), npub)
    }

    /// Fetches dependencies / requirements for the trust dialog.
    fn load_install_preview(&self, reference: &str) {
        let reference = reference.to_owned();
        self.spawn(move |client| {
            Event::InstallPreview(
                client
                    .call(
                        "GetPackage",
                        serde_json::json!({ "package": reference, "include_untrusted": true }),
                    )
                    .map_err(|e| UiError::from_anyhow(&e)),
            )
        });
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
                .map_err(|error| UiError::from_anyhow(&error));
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
                // Show publishers outside the trust list too (marked
                // "unknown") so installing from one can ask first.
                "include_untrusted": true,
            });
            let result = client
                .call("Search", params)
                .map(|value| value.as_array().cloned().unwrap_or_default())
                .map_err(|error| UiError::from_anyhow(&error));
            Event::Search(result)
        });
    }

    fn refresh_updates(&self) {
        self.spawn(|client| {
            let result = client
                .call("CheckUpdates", serde_json::json!({}))
                .map(|value| value.as_array().cloned().unwrap_or_default())
                .map_err(|error| UiError::from_anyhow(&error));
            Event::Updates(result)
        });
    }

    fn show_details(&self, package: &str) {
        let package = package.to_owned();
        self.spawn(move |client| {
            let result = client
                .call(
                    "GetPackage",
                    serde_json::json!({ "package": package, "include_untrusted": true }),
                )
                .map_err(|error| UiError::from_anyhow(&error));
            Event::Details(result)
        });
    }

    fn install(&mut self, package: &str, trust_once: Vec<String>, trust_first: Option<String>) {
        let package = package.to_owned();
        self.status = format!("Starting install of {package}...");
        self.spawn(move |client| {
            if let Some(publisher) = trust_first
                && let Err(error) = client.call(
                    "AddTrustedPublisher",
                    serde_json::json!({ "publisher": publisher }),
                )
            {
                return Event::ActionFinished {
                    action: "trust",
                    result: Err(UiError::from_anyhow(&error)),
                };
            }
            let params = serde_json::json!({
                "package": package, "async": true, "trust_once": trust_once
            });
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
                    result: Err(UiError::from_anyhow(&error)),
                },
            }
        });
    }

    /// Install button: go straight ahead for a trusted publisher, otherwise
    /// open the confirmation dialog first.
    fn request_install(&mut self, item: &Value) {
        let reference = package_reference(item);
        if item.get("trust").and_then(Value::as_str) == Some("trusted") {
            self.install(&reference, Vec::new(), None);
            return;
        }
        self.load_install_preview(&reference);
        self.request_profiles(vec![value_str(item, "publisher")]);
        self.pending_install = Some(PendingInstall {
            reference,
            publisher: value_str(item, "publisher"),
            name: value_str(item, "name"),
            version: value_str(item, "version"),
            preview: None,
        });
    }

    fn remove(&mut self, package: &str) {
        let package = package.to_owned();
        self.status = format!("Removing {package}...");
        self.spawn(move |client| {
            let result = client
                .call("Remove", serde_json::json!({ "package": package }))
                .map_err(|error| UiError::from_anyhow(&error));
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
                            result: Err(UiError::from_anyhow(&error)),
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
                Event::Installed(Err(error)) => self.fail("Loading installed packages", error),
                Event::Search(Ok(items)) => {
                    self.status = format!("{} result(s).", items.len());
                    let publishers = items
                        .iter()
                        .map(|item| value_str(item, "publisher"))
                        .collect();
                    self.search_results = items;
                    self.request_profiles(publishers);
                }
                Event::Search(Err(error)) => self.fail("Search", error),
                Event::Updates(Ok(items)) => {
                    self.status = format!("Checked {} installed package(s).", items.len());
                    self.updates = items;
                }
                Event::Updates(Err(error)) => self.fail("Checking for updates", error),
                Event::Details(Ok(value)) => self.details = Some(value),
                Event::Details(Err(error)) => self.fail("Loading details", error),
                Event::Config(Ok(value)) => {
                    self.relays_text = string_lines(&value, "relays");
                    self.blossom_text = string_lines(&value, "blossom");
                    let trusted = value
                        .get("trusted_publishers")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .map(|item| value_str(item, "publisher"))
                                .collect()
                        })
                        .unwrap_or_default();
                    self.config = Some(value);
                    self.request_profiles(trusted);
                }
                Event::Config(Err(error)) => self.fail("Updating settings", error),
                Event::InstallPreview(Ok(value)) => {
                    if let Some(pending) = &mut self.pending_install {
                        pending.preview = Some(value);
                    }
                }
                // The dialog still works without the extra detail.
                Event::InstallPreview(Err(_)) => {}
                Event::Profiles(Ok(items)) => {
                    for item in items {
                        self.identities.insert(value_str(&item, "publisher"), item);
                    }
                }
                // Names are optional; a failed lookup just leaves npubs showing.
                Event::Profiles(Err(_)) => {}
                Event::PublisherResolved(Ok(identity)) => {
                    self.identities
                        .insert(value_str(&identity, "publisher"), identity.clone());
                    self.trust_preview = Some(identity);
                }
                Event::PublisherResolved(Err(error)) => {
                    self.trust_preview = None;
                    self.fail("Looking up that publisher", error);
                }
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
                        Ok(value) => match UiError::from_failed_status(&value) {
                            Some(error) => self.fail(action, error),
                            None => {
                                self.status = format!("{action} finished.");
                                self.error = None;
                                self.refresh_installed();
                                self.refresh_config();
                            }
                        },
                        Err(error) => self.fail(action, error),
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

fn string_lines(config: &Value, field: &str) -> String {
    config
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn lines_to_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `npub1abcdefgh…uvwxyz`: enough to recognise, short enough to read.
fn short_npub(npub: &str) -> String {
    if npub.chars().count() <= 20 {
        return npub.to_owned();
    }
    let head: String = npub.chars().take(12).collect();
    let tail: String = npub
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

/// A verified NIP-05 name, a clearly marked self-reported claim, or just the
/// shortened npub. Only the first is ever shown without a caveat: a profile
/// can claim any name, so claims are labelled "(unverified)".
fn describe_publisher(identity: Option<&Value>, npub: &str) -> String {
    let short = short_npub(npub);
    let Some(identity) = identity else {
        return short;
    };
    let verified = identity.get("nip05_verified").and_then(Value::as_bool) == Some(true);
    if verified && let Some(label) = identity.get("label").and_then(Value::as_str) {
        return format!("{label} ✓");
    }
    let claimed = identity
        .get("claimed_name")
        .or_else(|| identity.get("nip05"))
        .and_then(Value::as_str);
    match claimed {
        Some(claimed) => format!("“{claimed}” (unverified) · {short}"),
        None => short,
    }
}

/// Search results carry the publisher as hex; people recognise npubs.
fn display_publisher(item: &Value) -> String {
    item.get("npub")
        .and_then(Value::as_str)
        .map_or_else(|| value_str(item, "publisher"), str::to_owned)
}

fn trust_badge(ui: &mut egui::Ui, trust: Option<&str>) {
    match trust {
        Some("trusted") => {
            ui.colored_label(egui::Color32::from_rgb(40, 150, 70), "● Trusted");
        }
        _ => {
            ui.colored_label(egui::Color32::from_rgb(200, 130, 20), "● Unknown publisher");
        }
    }
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
            self.show_error_banner(ui);
            if self.is_first_run() {
                self.show_welcome(ui);
                return;
            }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Installed, "Installed");
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.selectable_value(&mut self.tab, Tab::Updates, "Updates");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
            ui.separator();

            match self.tab {
                Tab::Installed => self.show_installed_tab(ui),
                Tab::Search => self.show_search_tab(ui),
                Tab::Updates => self.show_updates_tab(ui),
                Tab::Settings => self.show_settings_tab(ui),
            }
        });

        self.show_trust_dialog(&ui.ctx().clone());
    }
}

impl NpackGuiApp {
    fn is_first_run(&self) -> bool {
        !self.welcome_dismissed
            && self
                .config
                .as_ref()
                .and_then(|config| config.get("first_run"))
                .and_then(Value::as_bool)
                == Some(true)
    }

    fn show_error_banner(&mut self, ui: &mut egui::Ui) {
        let Some(error) = self.error.clone() else {
            return;
        };
        let mut dismiss = false;
        let mut open_settings = false;
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(200, 60, 40, 40))
            .inner_margin(egui::Margin::same(8))
            .corner_radius(4)
            .show(ui, |ui| {
                ui.label(error.headline());
                ui.horizontal(|ui| {
                    if error.suggests_settings() && ui.button("Open Settings").clicked() {
                        open_settings = true;
                    }
                    if ui.button("Dismiss").clicked() {
                        dismiss = true;
                    }
                    ui.toggle_value(&mut self.show_error_details, "Details");
                });
                if self.show_error_details {
                    ui.weak(&error.detail);
                }
            });
        ui.add_space(6.0);
        if open_settings {
            self.tab = Tab::Settings;
            self.welcome_dismissed = true;
            self.error = None;
        }
        if dismiss {
            self.error = None;
        }
    }

    fn show_welcome(&mut self, ui: &mut egui::Ui) {
        ui.heading("Welcome to npack");
        ui.add_space(6.0);
        ui.label(
            "npack installs software published by people you choose to trust. Every download is \
             checked against a signed checksum before anything is installed.",
        );
        ui.label(
            "Nobody is trusted to start with. You'll be asked before installing from a new \
             publisher, and you can change your mind any time in Settings.",
        );
        ui.add_space(10.0);
        if let Some(config) = &self.config {
            ui.strong("Where npack looks for software (relays)");
            for relay in lines_to_list(&string_lines(config, "relays")) {
                ui.label(format!("  {relay}"));
            }
            ui.add_space(4.0);
            ui.strong("Where it downloads from");
            for server in lines_to_list(&string_lines(config, "blossom")) {
                ui.label(format!("  {server}"));
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Use recommended settings").clicked() {
                self.save_config(None, None);
                self.welcome_dismissed = true;
            }
            if ui.button("Customise in Settings").clicked() {
                self.tab = Tab::Settings;
                self.welcome_dismissed = true;
            }
        });
    }

    fn show_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Trusted publishers");
        ui.label("npack only installs without asking from publishers on this list.");
        let publishers: Vec<(String, String)> = self
            .config
            .as_ref()
            .and_then(|config| config.get("trusted_publishers"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| (value_str(item, "publisher"), value_str(item, "npub")))
                    .collect()
            })
            .unwrap_or_default();
        if publishers.is_empty() {
            ui.weak("No one is trusted yet.");
        }
        let mut to_remove = None;
        for (publisher, npub) in &publishers {
            let text = self.publisher_text(publisher, npub);
            ui.horizontal(|ui| {
                ui.label(text).on_hover_text(npub);
                if ui.button("Stop trusting").clicked() {
                    to_remove = Some(publisher.clone());
                }
            });
        }
        if let Some(publisher) = to_remove {
            self.remove_trusted(publisher);
        }
        ui.horizontal(|ui| {
            ui.label("Add publisher (name@domain, npub or hex):");
            ui.text_edit_singleline(&mut self.trust_input);
            if ui.button("Look up").clicked() && !self.trust_input.trim().is_empty() {
                self.trust_preview = None;
                self.resolve_publisher(self.trust_input.trim().to_owned());
            }
        });
        if let Some(preview) = self.trust_preview.clone() {
            let hex = value_str(&preview, "publisher");
            let npub = value_str(&preview, "npub");
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(120, 120, 120, 30))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(4)
                .show(ui, |ui| {
                    ui.strong(describe_publisher(Some(&preview), &npub));
                    ui.label(&npub);
                    if preview.get("nip05_verified").and_then(Value::as_bool) != Some(true) {
                        ui.label(
                            "Names claimed in a profile can't be verified. Make sure this key \
                             matches what the publisher gave you.",
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Trust this publisher").clicked() {
                            self.add_trusted(hex.clone());
                            self.trust_preview = None;
                            self.trust_input.clear();
                        }
                        if ui.button("Cancel").clicked() {
                            self.trust_preview = None;
                        }
                    });
                });
        }

        ui.add_space(12.0);
        ui.heading("Network");
        ui.label("Relays (one per line):");
        ui.add(egui::TextEdit::multiline(&mut self.relays_text).desired_rows(3));
        ui.label("Download servers (one per line):");
        ui.add(egui::TextEdit::multiline(&mut self.blossom_text).desired_rows(3));
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                self.save_config(
                    Some(lines_to_list(&self.relays_text)),
                    Some(lines_to_list(&self.blossom_text)),
                );
            }
            if ui.button("Restore recommended").clicked() {
                self.save_config(Some(Vec::new()), Some(Vec::new()));
            }
        });
    }

    fn show_trust_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending_install else {
            return;
        };
        let mut choice = None;
        egui::Window::new("Install from a publisher you haven't trusted?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.strong(format!("{} {}", pending.name, pending.version));
                let npub = pending
                    .preview
                    .as_ref()
                    .and_then(|preview| preview.get("npub"))
                    .and_then(Value::as_str)
                    .unwrap_or(&pending.publisher);
                let identity = self.identities.get(&pending.publisher);
                ui.label(format!(
                    "Published by {}",
                    describe_publisher(identity, npub)
                ))
                .on_hover_text(npub);
                if identity
                    .and_then(|i| i.get("nip05_verified"))
                    .and_then(Value::as_bool)
                    != Some(true)
                {
                    ui.label(
                        "No verified name for this publisher. Any name shown above is \
                         self-reported and could be someone else's.",
                    );
                }
                ui.add_space(4.0);
                ui.label(
                    "npack has checked the signature and checksum. That proves who published \
                     these exact files, not that the software is safe.",
                );
                if let Some(preview) = &pending.preview {
                    let dependencies: Vec<String> = preview
                        .get("dependencies")
                        .and_then(Value::as_array)
                        .map(|items| items.iter().map(|d| value_str(d, "name")).collect())
                        .unwrap_or_default();
                    if !dependencies.is_empty() {
                        ui.label(format!("Also installs: {}", dependencies.join(", ")));
                    }
                    let requires: Vec<&str> = preview
                        .get("runtime_requires")
                        .and_then(Value::as_array)
                        .map(|items| items.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if !requires.is_empty() {
                        ui.label(format!("Needs on your system: {}", requires.join(", ")));
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Trust this publisher and install").clicked() {
                        choice = Some(TrustChoice::TrustAndInstall);
                    }
                    if ui.button("Install just this once").clicked() {
                        choice = Some(TrustChoice::InstallOnce);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(TrustChoice::Cancel);
                    }
                });
            });
        let Some(choice) = choice else {
            return;
        };
        let Some(pending) = self.pending_install.take() else {
            return;
        };
        match choice {
            TrustChoice::TrustAndInstall => {
                self.install(&pending.reference, Vec::new(), Some(pending.publisher));
            }
            TrustChoice::InstallOnce => {
                self.install(&pending.reference, vec![pending.publisher], None);
            }
            TrustChoice::Cancel => {}
        }
    }

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
                ui.label(format!(
                    "{} {}",
                    value_str(item, "name"),
                    value_str(item, "version")
                ));
                trust_badge(ui, item.get("trust").and_then(Value::as_str));
                ui.weak(
                    self.publisher_text(&value_str(item, "publisher"), &display_publisher(item)),
                );
                if ui.button("Details").clicked() {
                    to_show = Some(reference.clone());
                }
                if ui.button("Install").clicked() {
                    to_install = Some(item.clone());
                }
            });
        }
        if let Some(reference) = to_show {
            self.show_details(&reference);
        }
        if let Some(item) = to_install {
            self.request_install(&item);
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

#[cfg(test)]
mod tests {
    use super::*;

    const NPUB: &str = "npub10xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqpkge6d";

    #[test]
    fn only_verified_names_are_shown_without_a_caveat() {
        let verified = serde_json::json!({
            "nip05_verified": true, "label": "alice@example.com", "claimed_name": "Alice"
        });
        assert_eq!(
            describe_publisher(Some(&verified), NPUB),
            "alice@example.com ✓"
        );

        let claim = serde_json::json!({
            "nip05_verified": false, "nip05": "linus@kernel.org", "claimed_name": "Linus"
        });
        let text = describe_publisher(Some(&claim), NPUB);
        assert!(text.contains("(unverified)") && text.contains("Linus"));
        assert!(!text.contains('✓'));

        // An unverified record must never earn the tick, even with a label.
        let forged = serde_json::json!({"nip05_verified": false, "label": "x@y.com"});
        assert!(!describe_publisher(Some(&forged), NPUB).contains('✓'));
    }

    #[test]
    fn unknown_publishers_show_a_short_npub() {
        let text = describe_publisher(None, NPUB);
        assert!(text.starts_with("npub1") && text.contains('…') && text.len() < NPUB.len());
        let bare = serde_json::json!({"nip05_verified": false});
        assert_eq!(describe_publisher(Some(&bare), NPUB), text);
    }
}
