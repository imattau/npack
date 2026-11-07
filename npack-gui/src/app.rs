use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
    time::{SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use serde_json::Value;

use crate::{client::DaemonClient, friendly::UiError};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Installed,
    Browse,
    Updates,
    Health,
    Settings,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Installed,
        Tab::Browse,
        Tab::Updates,
        Tab::Health,
        Tab::Settings,
    ];

    fn name(self) -> &'static str {
        match self {
            Tab::Installed => "installed",
            Tab::Browse => "browse",
            Tab::Updates => "updates",
            Tab::Health => "health",
            Tab::Settings => "settings",
        }
    }

    fn from_name(name: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|tab| tab.name() == name)
    }
}

/// What a keyboard shortcut asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shortcut {
    GoTo(Tab),
    FocusSearch,
    Refresh,
    CloseDetails,
}

/// The shortcut for a key press, if any. Ctrl+1..5 switch tabs, Ctrl+F
/// searches, F5 or Ctrl+R refreshes the current tab and Escape closes the
/// details pane. Pure, so it is tested without a window.
fn shortcut_for(key: egui::Key, ctrl: bool) -> Option<Shortcut> {
    use egui::Key as K;
    match (key, ctrl) {
        (K::Num1, true) => Some(Shortcut::GoTo(Tab::Installed)),
        (K::Num2, true) => Some(Shortcut::GoTo(Tab::Browse)),
        (K::Num3, true) => Some(Shortcut::GoTo(Tab::Updates)),
        (K::Num4, true) => Some(Shortcut::GoTo(Tab::Health)),
        (K::Num5, true) => Some(Shortcut::GoTo(Tab::Settings)),
        (K::F, true) => Some(Shortcut::FocusSearch),
        (K::R, true) | (K::F5, _) => Some(Shortcut::Refresh),
        (K::Escape, false) => Some(Shortcut::CloseDetails),
        _ => None,
    }
}

/// How long to wait after the last keystroke before searching.
const SEARCH_DELAY: Duration = Duration::from_millis(350);

const BROWSE_PAGE: usize = 48;

/// Everything that shapes one browse request.
struct BrowseQuery {
    query: String,
    relays: Vec<String>,
    category: Option<String>,
    sort: &'static str,
    limit: usize,
    only_trusted: bool,
    refresh: bool,
}

/// The `Search` params and the filter-only params for `ListCategories` (the
/// category list must not shrink to the selected category, or the sidebar
/// would collapse to one entry).
fn browse_requests(browse: &BrowseQuery) -> (Value, Value) {
    // Show publishers outside the trust list too (marked "unknown") so
    // installing from one can ask first; only releases that fit this machine
    // are worth listing.
    let filters = serde_json::json!({
        "relay": browse.relays,
        "include_untrusted": true,
        "only_trusted": browse.only_trusted,
        "host_only": true,
    });
    let mut search = filters.clone();
    search["query"] = browse.query.clone().into();
    search["refresh"] = browse.refresh.into();
    search["sort"] = browse.sort.into();
    search["limit"] = browse.limit.into();
    if let Some(category) = &browse.category {
        search["category"] = category.clone().into();
    }
    (search, filters)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowseSort {
    Recent,
    Name,
}

impl BrowseSort {
    fn as_param(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::Name => "name",
        }
    }
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
    /// `ListCategories`: [{category, count}] for the sidebar.
    Categories(Result<Vec<Value>, UiError>),
    /// `ListRollbacks`: packages that can be switched back, and to what.
    Rollbacks(Result<Vec<Value>, UiError>),
    /// `GetHistory`: recent changes to installed packages, newest first.
    History(Result<Vec<Value>, UiError>),
    /// `RunDoctor`: the health check's findings.
    Health(Result<Value, UiError>),
    /// A verified icon fetched by npackd (`GetIcon`), keyed by its SHA-256.
    Icon {
        sha256: String,
        media_type: String,
        bytes: Vec<u8>,
    },
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

/// Sends events to the UI thread *and wakes it*. egui only redraws on input,
/// so without the wake-up a result that arrives while the mouse is still (a
/// click, then waiting) would sit unseen until the mouse moved.
#[derive(Clone)]
struct EventSender {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl EventSender {
    fn send(&self, event: Event) -> Result<(), mpsc::SendError<Event>> {
        let result = self.tx.send(event);
        self.ctx.request_repaint();
        result
    }
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
    remembered: crate::remember::Remembered,
    /// When the search box was last edited, while a search is waiting for the
    /// typing to pause.
    search_edited_at: Option<Instant>,
    focus_search: bool,
    socket_path_text: String,
    tab: Tab,

    installed: Vec<Value>,
    search_query: String,
    search_relays: String,
    search_results: Vec<Value>,
    /// Categories present in the catalogue, most populous first.
    categories: Vec<(String, u64)>,
    selected_category: Option<String>,
    browse_sort: BrowseSort,
    only_trusted: bool,
    /// How many results to ask for; "Show more" raises it.
    browse_limit: usize,
    /// The catalogue was fetched automatically after finding none, or the
    /// first browse was started; each happens at most once.
    auto_refreshed: bool,
    auto_browsed: bool,
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
    /// SHA-256 -> (bytes, file extension) for icons npackd has verified.
    icons: HashMap<String, (Arc<[u8]>, &'static str)>,
    icons_requested: HashSet<String>,
    /// The first automatic update check has been started.
    auto_checked: bool,
    /// The installed list has arrived at least once (until then the tab says
    /// "Loading", not "nothing installed").
    installed_loaded: bool,
    /// reference -> version it can be switched back to.
    rollbacks: HashMap<String, String>,
    /// Findings from the last health check, in report order.
    health: Vec<Value>,
    health_running: bool,
    /// Why the last health check couldn't run (shown in the tab, not as a banner).
    health_error: Option<UiError>,
    auto_health: bool,
    history: Vec<Value>,
    relays_text: String,
    blossom_text: String,

    tx: EventSender,
    rx: mpsc::Receiver<Event>,
}

impl NpackGuiApp {
    pub fn new(ctx: egui::Context) -> Self {
        let socket_path = DaemonClient::default_socket_path();
        let (tx, rx) = mpsc::channel();
        let tx = EventSender { tx, ctx };
        let remembered = crate::remember::load();
        let tab = remembered
            .tab
            .as_deref()
            .and_then(Tab::from_name)
            .unwrap_or(Tab::Installed);
        let app = Self {
            remembered,
            search_edited_at: None,
            focus_search: false,
            socket_path_text: socket_path.display().to_string(),
            client: Arc::new(DaemonClient::new(socket_path)),
            tab,
            installed: Vec::new(),
            search_query: String::new(),
            search_relays: String::new(),
            search_results: Vec::new(),
            categories: Vec::new(),
            selected_category: None,
            browse_sort: BrowseSort::Recent,
            only_trusted: false,
            browse_limit: BROWSE_PAGE,
            auto_refreshed: false,
            auto_browsed: false,
            updates: Vec::new(),
            details: None,
            status: "Connecting...".to_owned(),
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
            icons: HashMap::new(),
            icons_requested: HashSet::new(),
            auto_checked: false,
            installed_loaded: false,
            rollbacks: HashMap::new(),
            health: Vec::new(),
            health_running: false,
            health_error: None,
            auto_health: false,
            history: Vec::new(),
            relays_text: String::new(),
            blossom_text: String::new(),
            tx,
            rx,
        };
        app.start_up();
        app
    }

    /// Starts the background service if it isn't running (a normal person
    /// should never have to), then loads the settings. Off the UI thread.
    fn start_up(&self) {
        self.spawn(|client| {
            if let Err(error) = client.ensure_daemon(Duration::from_secs(8)) {
                return Event::Config(Err(UiError::from_anyhow(&error)));
            }
            Event::Config(
                client
                    .call("GetConfig", serde_json::json!({}))
                    .map_err(|error| UiError::from_anyhow(&error)),
            )
        });
    }

    /// Reconnects using the socket path in Settings, and reruns the checks
    /// that happen once at start.
    fn reconnect(&mut self) {
        self.client = Arc::new(DaemonClient::new(PathBuf::from(&self.socket_path_text)));
        self.error = None;
        self.status = "Connecting...".to_owned();
        self.auto_checked = false;
        self.auto_browsed = false;
        self.auto_health = false;
        self.start_up();
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

    /// Fetches icons for results that have one, one at a time in the
    /// background (at most 24 per call). npackd checks each against the hash
    /// the publisher signed; anything that fails simply shows no icon.
    fn request_icons(&mut self, items: &[Value]) {
        let wanted: Vec<(String, String)> = items
            .iter()
            .filter_map(|item| {
                Some((
                    value_str(item, "release_event_id"),
                    icon_sha(item)?.to_owned(),
                ))
            })
            .filter(|(_, sha)| self.icons_requested.insert(sha.clone()))
            .take(24)
            .collect();
        if wanted.is_empty() {
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            use base64::Engine;
            for (release_event_id, sha256) in wanted {
                let Ok(reply) = client.call(
                    "GetIcon",
                    serde_json::json!({ "release_event_id": release_event_id }),
                ) else {
                    continue;
                };
                let Some(bytes) = reply
                    .get("data")
                    .and_then(Value::as_str)
                    .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
                else {
                    continue;
                };
                let _ = tx.send(Event::Icon {
                    sha256,
                    media_type: value_str(&reply, "media_type"),
                    bytes,
                });
            }
        });
    }

    /// The icon for a listing, if npackd has delivered it.
    fn icon_image(&self, item: &Value) -> Option<egui::Image<'static>> {
        let sha = icon_sha(item)?;
        let (bytes, extension) = self.icons.get(sha)?;
        Some(
            egui::Image::from_bytes(
                format!("bytes://{sha}.{extension}"),
                egui::load::Bytes::Shared(bytes.clone()),
            )
            .fit_to_exact_size(egui::vec2(ICON_SIZE, ICON_SIZE)),
        )
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

    /// Lists the catalogue with the current query, category, sort and filters
    /// (an empty query browses everything), then refreshes the category list.
    /// With `refresh`, the catalogue is rebuilt from the relays first.
    fn browse(&mut self, refresh: bool) {
        if refresh {
            self.status = "Fetching the software catalogue...".to_owned();
        }
        let query = self.search_query.trim().to_owned();
        let relays: Vec<String> = self
            .search_relays
            .split(',')
            .map(str::trim)
            .filter(|relay| !relay.is_empty())
            .map(str::to_owned)
            .collect();
        let category = self.selected_category.clone();
        let sort = self.browse_sort.as_param();
        let limit = self.browse_limit;
        let only_trusted = self.only_trusted;
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (search, filters) = browse_requests(&BrowseQuery {
                query,
                relays,
                category,
                sort,
                limit,
                only_trusted,
                refresh,
            });
            let result = client
                .call("Search", search)
                .map(|value| value.as_array().cloned().unwrap_or_default())
                .map_err(|error| UiError::from_anyhow(&error));
            let succeeded = result.is_ok();
            let _ = tx.send(Event::Search(result));
            if succeeded {
                let _ = tx.send(Event::Categories(
                    client
                        .call("ListCategories", filters)
                        .map(|value| value.as_array().cloned().unwrap_or_default())
                        .map_err(|error| UiError::from_anyhow(&error)),
                ));
            }
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

    /// Updates one package, or everything installed, as a background
    /// transaction (same progress flow as an install).
    fn update(&mut self, package: Option<String>) {
        self.status = match &package {
            Some(package) => format!("Starting update of {package}..."),
            None => "Starting update of all packages...".to_owned(),
        };
        self.spawn(move |client| {
            let mut params = serde_json::json!({ "async": true });
            if let Some(package) = package {
                params["package"] = package.into();
            }
            match client.call("Update", params) {
                Ok(value) => match value.get("transaction_id").and_then(Value::as_u64) {
                    Some(id) => Event::TransactionStarted {
                        action: "update",
                        id,
                    },
                    None => Event::ActionFinished {
                        action: "update",
                        result: Ok(value),
                    },
                },
                Err(error) => Event::ActionFinished {
                    action: "update",
                    result: Err(UiError::from_anyhow(&error)),
                },
            }
        });
    }

    fn refresh_rollbacks_and_history(&self) {
        self.spawn(|client| {
            Event::Rollbacks(
                client
                    .call("ListRollbacks", serde_json::json!({}))
                    .map(|value| value.as_array().cloned().unwrap_or_default())
                    .map_err(|error| UiError::from_anyhow(&error)),
            )
        });
        self.spawn(|client| {
            Event::History(
                client
                    .call("GetHistory", serde_json::json!({ "limit": 20 }))
                    .map(|value| value.as_array().cloned().unwrap_or_default())
                    .map_err(|error| UiError::from_anyhow(&error)),
            )
        });
    }

    /// Switches a package back to the version it replaced (works offline).
    fn rollback(&mut self, package: &str) {
        let package = package.to_owned();
        self.status = format!("Switching {package} back...");
        self.spawn(move |client| Event::ActionFinished {
            action: "rollback",
            result: client
                .call("Rollback", serde_json::json!({ "package": package }))
                .map_err(|error| UiError::from_anyhow(&error)),
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
                    let publishers = items
                        .iter()
                        .map(|item| value_str(item, "publisher"))
                        .collect();
                    self.installed = items;
                    self.installed_loaded = true;
                    self.request_profiles(publishers);
                    self.refresh_rollbacks_and_history();
                }
                Event::Rollbacks(Ok(items)) => {
                    self.rollbacks = items
                        .iter()
                        .map(|item| {
                            (
                                value_str(item, "reference"),
                                value_str(item, "previous_version"),
                            )
                        })
                        .collect();
                }
                Event::History(Ok(items)) => self.history = items,
                Event::Health(Ok(reply)) => {
                    self.health_running = false;
                    self.health_error = None;
                    self.health = reply
                        .get("findings")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                }
                Event::Health(Err(error)) => {
                    self.health_running = false;
                    self.health_error = Some(error);
                }
                // Rollback and history are extras; failing to load them must
                // not raise an error banner over the installed list.
                Event::Rollbacks(Err(_)) | Event::History(Err(_)) => {}
                Event::Installed(Err(error)) => self.fail("Loading installed packages", error),
                Event::Search(Ok(items)) => {
                    self.status = format!("{} result(s).", items.len());
                    let publishers = items
                        .iter()
                        .map(|item| value_str(item, "publisher"))
                        .collect();
                    self.request_icons(&items);
                    self.search_results = items;
                    self.request_profiles(publishers);
                }
                Event::Search(Err(error)) => {
                    if error.code.as_deref() == Some("no_catalogue") && !self.auto_refreshed {
                        // First time here: there is nothing to browse until
                        // the catalogue has been downloaded, so do that now
                        // instead of showing an error.
                        self.auto_refreshed = true;
                        self.browse(true);
                    } else {
                        self.fail("Browsing", error);
                    }
                }
                Event::Categories(Ok(items)) => {
                    self.categories = items
                        .iter()
                        .map(|item| {
                            (
                                value_str(item, "category"),
                                item.get("count").and_then(Value::as_u64).unwrap_or(0),
                            )
                        })
                        .collect();
                }
                // The sidebar is a convenience; results still show without it.
                Event::Categories(Err(_)) => {}
                Event::Updates(Ok(items)) => {
                    self.status = format!("Checked {} installed package(s).", items.len());
                    self.updates = items;
                }
                Event::Updates(Err(error)) => self.fail("Checking for updates", error),
                Event::Details(Ok(value)) => {
                    self.request_icons(std::slice::from_ref(&value));
                    self.details = Some(value);
                }
                Event::Icon {
                    sha256,
                    media_type,
                    bytes,
                } => {
                    let extension = if media_type == "image/svg+xml" {
                        "svg"
                    } else {
                        "png"
                    };
                    self.icons.insert(sha256, (bytes.into(), extension));
                }
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
                    if self.status == "Connecting..." {
                        self.status = "Ready.".to_owned();
                    }
                    let first_run = value.get("first_run").and_then(Value::as_bool) == Some(true);
                    self.config = Some(value);
                    self.request_profiles(trusted);
                    // Look for updates (and revoked packages) once at start,
                    // as soon as the app is actually set up.
                    if !first_run && !self.auto_checked {
                        self.auto_checked = true;
                        self.refresh_installed();
                        self.refresh_updates();
                    }
                    if !first_run && !self.auto_browsed {
                        self.auto_browsed = true;
                        self.browse(false);
                    }
                    if !first_run && !self.auto_health {
                        self.auto_health = true;
                        self.run_health_check();
                    }
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
                                self.refresh_updates();
                            }
                        },
                        Err(error) => self.fail(action, error),
                    }
                }
            }
        }
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

const ICON_SIZE: f32 = 40.0;

/// (problems that stop npack working, warnings) in a health check's findings.
fn health_counts(findings: &[Value]) -> (usize, usize) {
    let count = |level: &str| {
        findings
            .iter()
            .filter(|finding| finding.get("level").and_then(Value::as_str) == Some(level))
            .count()
    };
    (count("fail"), count("warn"))
}

fn health_headline(failures: usize, warnings: usize) -> String {
    match (failures, warnings) {
        (0, 0) => "Everything looks good.".to_owned(),
        (0, warnings) => format!("npack is working. {warnings} thing(s) to look at."),
        (failures, 0) => format!("{failures} problem(s) stop npack working properly."),
        (failures, warnings) => {
            format!(
                "{failures} problem(s) stop npack working properly, plus {warnings} thing(s) to look at."
            )
        }
    }
}

/// Findings grouped by area, in the order each area first appears.
fn group_findings_by_area(findings: &[Value]) -> Vec<(String, Vec<&Value>)> {
    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
    for finding in findings {
        let area = value_str(finding, "area");
        match groups.iter_mut().find(|(name, _)| *name == area) {
            Some((_, members)) => members.push(finding),
            None => groups.push((area, vec![finding])),
        }
    }
    groups
}

/// The marker and colour for a finding's level.
fn finding_marker(finding: &Value) -> (&'static str, egui::Color32) {
    // A coloured dot is drawn; the word is for people who can't tell the
    // colours apart, and only on the levels that need attention. (egui's
    // built-in fonts have no tick, cross or arrow glyphs, so symbols are
    // drawn or spelled out rather than typed.)
    match finding.get("level").and_then(Value::as_str) {
        Some("ok") => ("", egui::Color32::from_rgb(40, 150, 70)),
        Some("warn") => ("Warning", egui::Color32::from_rgb(200, 130, 20)),
        Some("fail") => ("Problem", egui::Color32::from_rgb(200, 50, 40)),
        _ => ("", egui::Color32::GRAY),
    }
}

/// A small filled circle, drawn rather than typed so it needs no font glyph.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
}

/// "5 minutes ago", "3 days ago".
fn describe_age(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    let (count, unit) = match seconds {
        0..=59 => return "just now".to_owned(),
        60..=3599 => (seconds / 60, "minute"),
        3600..=86_399 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

/// One line of the history list, e.g. "2 hours ago: upgraded x/app 1.0.0 to 1.1.0".
fn history_line(entry: &Value, now: u64) -> String {
    let version = |field: &str| {
        entry
            .get(field)
            .and_then(|v| v.get("version"))
            .and_then(Value::as_str)
    };
    let verb = match entry
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("changed")
    {
        "install" => "installed",
        "upgrade" => "upgraded",
        "downgrade" => "downgraded",
        "reinstall" => "reinstalled",
        "rollback" => "switched back",
        "remove" => "removed",
        other => other,
    };
    let change = match (version("from"), version("to")) {
        (Some(from), Some(to)) => format!("{from} to {to}"),
        (None, Some(to)) => to.to_owned(),
        (Some(from), None) => from.to_owned(),
        (None, None) => String::new(),
    };
    let age = describe_age(now, entry.get("at").and_then(Value::as_u64).unwrap_or(now));
    format!("{age}: {verb} {} {change}", value_str(entry, "reference"))
}

/// A listing field the publisher wrote (already cleaned by npackd). Shown as
/// plain text only: never interpreted, never fetched.
fn app_text<'a>(item: &'a Value, field: &str) -> Option<&'a str> {
    item.get("app")?
        .get(field)?
        .as_str()
        .filter(|text| !text.is_empty())
}

fn app_categories(item: &Value) -> Vec<&str> {
    item.get("app")
        .and_then(|app| app.get("categories"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The SHA-256 of the listing's icon, which identifies it in the icon cache.
fn icon_sha(item: &Value) -> Option<&str> {
    item.get("app")?.get("icon")?.get("sha256")?.as_str()
}

/// The right-hand details pane: the listing first (what a person wants to
/// know), then the technical facts, with the raw data tucked away.
fn details_view(
    ui: &mut egui::Ui,
    value: &Value,
    publisher: &str,
    icon: Option<egui::Image<'static>>,
) {
    ui.horizontal(|ui| {
        show_icon(ui, icon, &value_str(value, "name"));
        ui.vertical(|ui| {
            ui.heading(value_str(value, "name"));
            ui.label(value_str(value, "version"));
        });
    });
    if let Some(summary) = app_text(value, "summary") {
        ui.strong(summary);
    }
    ui.horizontal(|ui| {
        trust_badge(ui, value.get("trust").and_then(Value::as_str));
        ui.weak(publisher);
    });
    if let Some(description) = app_text(value, "description") {
        ui.add_space(6.0);
        ui.label(description);
    }
    ui.add_space(6.0);
    if let Some(license) = app_text(value, "license") {
        ui.label(format!("License: {license}"));
    }
    let categories = app_categories(value);
    if !categories.is_empty() {
        ui.label(format!("Categories: {}", categories.join(", ")));
    }
    if let Some(date) = app_text(value, "release_date") {
        ui.label(format!("Released: {date}"));
    }
    if let Some(homepage) = app_text(value, "homepage")
        && (homepage.starts_with("https://") || homepage.starts_with("http://"))
    {
        // The full address is the link text, so it can't pose as something
        // else; nothing is opened until the person clicks.
        ui.hyperlink_to(homepage, homepage);
    }
    let dependencies: Vec<String> = value
        .get("dependencies")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|d| value_str(d, "name")).collect())
        .unwrap_or_default();
    if !dependencies.is_empty() {
        ui.label(format!("Also installs: {}", dependencies.join(", ")));
    }
    ui.add_space(6.0);
    ui.collapsing("Technical details", |ui| {
        ui.label(serde_json::to_string_pretty(value).unwrap_or_default());
    });
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
        return format!("{label} (verified)");
    }
    let claimed = identity
        .get("claimed_name")
        .or_else(|| identity.get("nip05"))
        .and_then(Value::as_str);
    match claimed {
        Some(claimed) => format!("\"{claimed}\" (unverified) · {short}"),
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
    let (color, text) = match trust {
        Some("trusted") => (egui::Color32::from_rgb(40, 150, 70), "Trusted"),
        _ => (egui::Color32::from_rgb(200, 130, 20), "Unknown publisher"),
    };
    status_dot(ui, color);
    ui.colored_label(color, text);
}

/// Muted tile colours for the stand-in icon.
const TILE_COLORS: [(u8, u8, u8); 8] = [
    (86, 116, 163),
    (96, 140, 110),
    (160, 110, 90),
    (130, 100, 150),
    (150, 130, 80),
    (80, 140, 150),
    (160, 95, 115),
    (110, 120, 130),
];

/// Which tile colour a package gets: stable for a given name.
fn tile_color_index(name: &str) -> usize {
    name.bytes().fold(7u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    }) as usize
        % TILE_COLORS.len()
}

/// The letter shown on a stand-in icon.
fn tile_letter(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

/// A package's icon, or a coloured tile with its initial when it has none (or
/// it hasn't arrived yet), so rows line up and nothing is left as a blank gap.
fn show_icon(ui: &mut egui::Ui, icon: Option<egui::Image<'static>>, name: &str) {
    match icon {
        Some(icon) => {
            ui.add(icon);
        }
        None => {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ICON_SIZE, ICON_SIZE), egui::Sense::hover());
            let (r, g, b) = TILE_COLORS[tile_color_index(name)];
            ui.painter()
                .rect_filled(rect, 8, egui::Color32::from_rgb(r, g, b));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                tile_letter(name),
                egui::FontId::proportional(22.0),
                egui::Color32::WHITE,
            );
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
        self.handle_shortcuts(ctx);
        self.remember_changes(ctx);
        // Redraw regularly while a background call or transaction is in
        // flight, so polling results and progress show up promptly.
        if self.active_transaction.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });

        // The details pane only appears while a package is selected, and only
        // on the tabs that list packages, so it never squeezes the others.
        let mut close_details = false;
        if self.details.is_some() && matches!(self.tab, Tab::Browse | Tab::Installed) {
            egui::Panel::right("details")
                .default_size(300.0)
                .size_range(240.0..=460.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Details");
                        if ui.button("Close").clicked() {
                            close_details = true;
                        }
                    });
                    if let Some(value) = &self.details {
                        let publisher = self.publisher_text(
                            &value_str(value, "publisher"),
                            &display_publisher(value),
                        );
                        details_view(ui, value, &publisher, self.icon_image(value));
                    }
                });
        }
        if close_details {
            self.details = None;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            self.show_error_banner(ui);
            self.show_revoked_warning(ui);
            self.show_health_warning(ui);
            if self.is_first_run() {
                self.show_welcome(ui);
                return;
            }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Installed, "Installed");
                ui.selectable_value(&mut self.tab, Tab::Browse, "Browse");
                let updates_label = match update_counts(&self.updates).0 {
                    0 => "Updates".to_owned(),
                    count => format!("Updates ({count})"),
                };
                ui.selectable_value(&mut self.tab, Tab::Updates, updates_label);
                let health_label = match health_counts(&self.health) {
                    (0, 0) => "Health".to_owned(),
                    (0, warnings) => format!("Health ({warnings})"),
                    _ => "Health (!)".to_owned(),
                };
                ui.selectable_value(&mut self.tab, Tab::Health, health_label);
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
            ui.separator();

            match self.tab {
                Tab::Installed => self.show_installed_tab(ui),
                Tab::Browse => self.show_browse_tab(ui),
                Tab::Updates => self.show_updates_tab(ui),
                Tab::Health => self.show_health_tab(ui),
                Tab::Settings => self.show_settings_tab(ui),
            }
        });

        self.show_trust_dialog(&ui.ctx().clone());
    }
}

impl NpackGuiApp {
    /// Keyboard shortcuts (see `shortcut_for`). Ignored on the welcome screen,
    /// where there is nothing to switch to yet.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.is_first_run() {
            return;
        }
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::F,
            egui::Key::R,
            egui::Key::F5,
            egui::Key::Escape,
        ];
        let mut requested = Vec::new();
        ctx.input_mut(|input| {
            for key in keys {
                for ctrl in [true, false] {
                    let modifiers = if ctrl {
                        egui::Modifiers::COMMAND
                    } else {
                        egui::Modifiers::NONE
                    };
                    // Escape only counts while there is something to close,
                    // so it still cancels a text edit or dialog otherwise.
                    if shortcut_for(key, ctrl).is_some()
                        && (key != egui::Key::Escape || self.details.is_some())
                        && input.consume_key(modifiers, key)
                        && let Some(shortcut) = shortcut_for(key, ctrl)
                    {
                        requested.push(shortcut);
                    }
                }
            }
        });
        for shortcut in requested {
            match shortcut {
                Shortcut::GoTo(tab) => self.tab = tab,
                Shortcut::FocusSearch => {
                    self.tab = Tab::Browse;
                    self.focus_search = true;
                }
                Shortcut::CloseDetails => self.details = None,
                Shortcut::Refresh => match self.tab {
                    Tab::Installed => self.refresh_installed(),
                    Tab::Browse => self.browse(false),
                    Tab::Updates => self.refresh_updates(),
                    Tab::Health => self.run_health_check(),
                    Tab::Settings => self.refresh_config(),
                },
            }
        }
    }

    /// Saves the current tab and window size when they change, so the next
    /// start looks the same.
    fn remember_changes(&mut self, ctx: &egui::Context) {
        let size = ctx.input(|input| input.viewport().inner_rect.map(|r| [r.width(), r.height()]));
        let mut changed = false;
        if self.remembered.tab.as_deref() != Some(self.tab.name()) {
            self.remembered.tab = Some(self.tab.name().to_owned());
            changed = true;
        }
        if let Some(size) = size
            && self.remembered.size_changed(size)
        {
            self.remembered.size = Some(size);
            changed = true;
        }
        if changed {
            crate::remember::save(&self.remembered);
        }
    }

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
        let mut reconnect = false;
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(200, 60, 40, 40))
            .inner_margin(egui::Margin::same(8))
            .corner_radius(4)
            .show(ui, |ui| {
                ui.label(error.headline());
                ui.horizontal(|ui| {
                    if error.code.as_deref() == Some("daemon_unreachable")
                        && ui.button("Try again").clicked()
                    {
                        reconnect = true;
                    }
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
        if reconnect {
            self.reconnect();
        }
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

    /// Runs the daemon's health check in the background (same checks as
    /// `npack doctor`). Ignored while one is already running.
    fn run_health_check(&mut self) {
        if self.health_running {
            return;
        }
        self.health_running = true;
        self.spawn(|client| {
            Event::Health(
                client
                    .call("RunDoctor", serde_json::json!({}))
                    .map_err(|error| UiError::from_anyhow(&error)),
            )
        });
    }

    /// A strip on every tab while the last check found something that stops
    /// npack working (warnings alone only show on the tab's badge).
    fn show_health_warning(&mut self, ui: &mut egui::Ui) {
        if health_counts(&self.health).0 == 0 {
            return;
        }
        let mut open = false;
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(220, 60, 40, 50))
            .inner_margin(egui::Margin::same(8))
            .corner_radius(4)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("npack found a problem that stops it working properly.");
                    if ui.button("See what to do").clicked() {
                        open = true;
                    }
                });
            });
        ui.add_space(6.0);
        if open {
            self.tab = Tab::Health;
        }
    }

    fn show_health_tab(&mut self, ui: &mut egui::Ui) {
        let mut recheck = false;
        ui.horizontal(|ui| {
            ui.heading("Health check");
            if self.health_running {
                ui.spinner();
                ui.weak("Checking...");
            } else if ui.button("Check again").clicked() {
                recheck = true;
            }
        });
        if let Some(error) = &self.health_error {
            ui.colored_label(egui::Color32::from_rgb(200, 50, 40), error.headline());
            ui.weak(&error.detail);
        }
        if self.health.is_empty() && self.health_error.is_none() && !self.health_running {
            ui.weak("Not checked yet.");
        }
        if !self.health.is_empty() {
            let (failures, warnings) = health_counts(&self.health);
            ui.label(health_headline(failures, warnings));
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("health")
            .show(ui, |ui| {
                for (area, findings) in group_findings_by_area(&self.health) {
                    ui.add_space(6.0);
                    ui.strong(area);
                    for finding in findings {
                        let (word, color) = finding_marker(finding);
                        ui.horizontal_wrapped(|ui| {
                            status_dot(ui, color);
                            if !word.is_empty() {
                                ui.colored_label(color, word);
                            }
                            ui.label(value_str(finding, "title"));
                        });
                        if let Some(detail) = finding.get("detail").and_then(Value::as_str) {
                            for line in detail.lines() {
                                ui.indent(("detail", line), |ui| {
                                    ui.weak(line);
                                });
                            }
                        }
                        if let Some(fix) = finding.get("fix").and_then(Value::as_str) {
                            ui.indent(("fix", fix), |ui| {
                                ui.label(format!("Fix: {fix}"));
                            });
                        }
                    }
                }
            });
        if recheck {
            self.run_health_check();
        }
    }

    fn show_settings_tab(&mut self, ui: &mut egui::Ui) {
        let mut reconnect = false;
        ui.collapsing("Advanced", |ui| {
            ui.label("npack's background service listens on this socket.");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.socket_path_text);
                if ui.button("Reconnect").clicked() {
                    reconnect = true;
                }
            });
        });
        ui.add_space(8.0);
        if reconnect {
            self.reconnect();
        }
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
                    let claims_a_name = identity
                        .and_then(|i| i.get("claimed_name").or_else(|| i.get("nip05")))
                        .is_some();
                    ui.label(if claims_a_name {
                        "No verified name for this publisher. The name shown above is \
                         self-reported and could be someone else's."
                    } else {
                        "This publisher has no verified name. If you know who it should \
                         be, check that the key above matches."
                    });
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
        let mut to_rollback = None;
        let mut go_browse = false;
        if !self.installed_loaded {
            ui.add_space(8.0);
            ui.weak("Loading...");
        } else if self.installed.is_empty() {
            ui.add_space(8.0);
            ui.label("Nothing is installed yet.");
            if ui.button("Browse software").clicked() {
                go_browse = true;
            }
        }
        for item in &self.installed {
            let reference = package_reference(item);
            ui.horizontal(|ui| {
                show_icon(ui, None, &value_str(item, "name"));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(value_str(item, "name"));
                        ui.label(value_str(item, "version"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Remove").clicked() {
                                to_remove = Some(reference.clone());
                            }
                            if let Some(previous) = self.rollbacks.get(&reference)
                                && self.active_transaction.is_none()
                                && ui
                                    .button(format!("Switch back to {previous}"))
                                    .on_hover_text(
                                        "Reinstalls the version this one replaced, from the copy \
                                         npack kept. No download needed.",
                                    )
                                    .clicked()
                            {
                                to_rollback = Some(reference.clone());
                            }
                            if ui.button("Details").clicked() {
                                to_show = Some(reference.clone());
                            }
                        });
                    });
                    ui.weak(
                        self.publisher_text(
                            &value_str(item, "publisher"),
                            &display_publisher(item),
                        ),
                    );
                });
            });
            ui.add_space(6.0);
        }
        if !self.history.is_empty() {
            ui.add_space(8.0);
            ui.collapsing("History", |ui| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default();
                for entry in &self.history {
                    ui.label(history_line(entry, now));
                }
            });
        }
        if let Some(reference) = to_show {
            self.show_details(&reference);
        }
        if go_browse {
            self.tab = Tab::Browse;
        }
        if let Some(reference) = to_rollback {
            self.rollback(&reference);
        }
        if let Some(reference) = to_remove {
            self.remove(&reference);
        }
    }

    fn show_browse_tab(&mut self, ui: &mut egui::Ui) {
        let mut rerun = false;
        let mut refresh = false;
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .desired_width(340.0)
                    .hint_text("Search by name, summary or category (Ctrl+F)"),
            );
            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }
            // Search as you type, once the typing pauses.
            if response.changed() {
                self.search_edited_at = Some(Instant::now());
            }
            if let Some(edited) = self.search_edited_at {
                let waited = edited.elapsed();
                if waited >= SEARCH_DELAY {
                    self.search_edited_at = None;
                    self.browse_limit = BROWSE_PAGE;
                    rerun = true;
                } else {
                    ui.ctx().request_repaint_after(SEARCH_DELAY - waited);
                }
            }
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Search").clicked() || enter {
                self.search_edited_at = None;
                self.browse_limit = BROWSE_PAGE;
                rerun = true;
            }
            if ui
                .button("Refresh catalogue")
                .on_hover_text("Download the latest list of software from the relays")
                .clicked()
            {
                refresh = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Sort:");
            let before = (self.browse_sort, self.only_trusted);
            ui.selectable_value(
                &mut self.browse_sort,
                BrowseSort::Recent,
                "Recently updated",
            );
            ui.selectable_value(&mut self.browse_sort, BrowseSort::Name, "Name");
            ui.separator();
            ui.checkbox(&mut self.only_trusted, "Only publishers I trust");
            if before != (self.browse_sort, self.only_trusted) {
                self.browse_limit = BROWSE_PAGE;
                rerun = true;
            }
        });
        ui.collapsing("Relays", |ui| {
            ui.horizontal(|ui| {
                ui.label("Comma-separated; blank uses your settings:");
                ui.text_edit_singleline(&mut self.search_relays);
            });
        });
        ui.separator();

        let mut picked_category: Option<Option<String>> = None;
        let mut to_install = None;
        let mut to_show = None;
        let mut show_more = false;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(180.0, ui.available_height()),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.strong("Categories");
                    egui::ScrollArea::vertical()
                        .id_salt("categories")
                        .show(ui, |ui| {
                            if ui
                                .selectable_label(self.selected_category.is_none(), "All")
                                .clicked()
                            {
                                picked_category = Some(None);
                            }
                            for (category, count) in &self.categories {
                                let selected = self
                                    .selected_category
                                    .as_deref()
                                    .is_some_and(|chosen| chosen.eq_ignore_ascii_case(category));
                                if ui
                                    .selectable_label(selected, format!("{category} ({count})"))
                                    .clicked()
                                {
                                    picked_category = Some(Some(category.clone()));
                                }
                            }
                            if self.categories.is_empty() {
                                ui.weak("None yet");
                            }
                        });
                },
            );
            ui.separator();
            ui.vertical(|ui| {
                let heading = match (&self.selected_category, self.search_query.trim()) {
                    (Some(category), "") => category.to_string(),
                    (Some(category), query) => format!("{category}: \"{query}\""),
                    (None, "") => "All software".to_owned(),
                    (None, query) => format!("Results for \"{query}\""),
                };
                ui.heading(heading);
                ui.weak(format!("{} shown", self.search_results.len()));
                egui::ScrollArea::vertical()
                    .id_salt("results")
                    .show(ui, |ui| {
                        if self.search_results.is_empty() {
                            ui.weak("Nothing to show. Try another search, or Refresh catalogue.");
                        }
                        for item in &self.search_results {
                            let reference = package_reference(item);
                            ui.horizontal(|ui| {
                                show_icon(ui, self.icon_image(item), &value_str(item, "name"));
                                ui.vertical(|ui| {
                                    // The buttons sit at the right edge of the
                                    // row so they stay visible however narrow
                                    // the window is.
                                    ui.horizontal(|ui| {
                                        ui.strong(value_str(item, "name"));
                                        ui.label(value_str(item, "version"));
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add(crate::style::primary_button("Install"))
                                                    .clicked()
                                                {
                                                    to_install = Some(item.clone());
                                                }
                                                if ui.button("Details").clicked() {
                                                    to_show = Some(reference.clone());
                                                }
                                            },
                                        );
                                    });
                                    if let Some(summary) = app_text(item, "summary") {
                                        ui.label(summary);
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        let categories = app_categories(item);
                                        if !categories.is_empty() {
                                            ui.weak(categories.join(" · "));
                                        }
                                        trust_badge(ui, item.get("trust").and_then(Value::as_str));
                                        ui.weak(self.publisher_text(
                                            &value_str(item, "publisher"),
                                            &display_publisher(item),
                                        ));
                                    });
                                });
                            });
                            ui.add_space(6.0);
                        }
                        if self.search_results.len() >= self.browse_limit
                            && ui.button("Show more").clicked()
                        {
                            show_more = true;
                        }
                    });
            });
        });

        if let Some(choice) = picked_category {
            self.selected_category = choice;
            self.browse_limit = BROWSE_PAGE;
            rerun = true;
        }
        if show_more {
            self.browse_limit += BROWSE_PAGE;
            rerun = true;
        }
        if refresh {
            self.auto_refreshed = true;
            self.browse(true);
        } else if rerun {
            self.browse(false);
        }
        if let Some(reference) = to_show {
            self.show_details(&reference);
        }
        if let Some(item) = to_install {
            self.request_install(&item);
        }
    }

    fn show_updates_tab(&mut self, ui: &mut egui::Ui) {
        let (available, _) = update_counts(&self.updates);
        let mut to_update = None;
        let mut update_all = false;
        ui.horizontal(|ui| {
            if ui.button("Check for updates").clicked() {
                self.refresh_updates();
            }
            if available > 0
                && self.active_transaction.is_none()
                && ui.button(format!("Update all ({available})")).clicked()
            {
                update_all = true;
            }
        });
        ui.separator();
        if self.updates.is_empty() {
            if self.installed_loaded && self.installed.is_empty() {
                ui.weak("Nothing is installed yet, so there are no updates to check.");
            } else {
                ui.weak("Not checked yet.");
            }
        }
        for item in &self.updates {
            let reference = value_str(item, "reference");
            let current = value_str(item, "current_version");
            let available = item
                .get("available_version")
                .and_then(Value::as_str)
                .map(str::to_owned);
            ui.horizontal(|ui| {
                if let Some(reason) = item.get("revoked").and_then(Value::as_str) {
                    ui.colored_label(egui::Color32::from_rgb(200, 50, 40), "Revoked");
                    ui.label(format!("{reference} {current}"));
                    ui.weak(format!("The publisher withdrew this version: {reason}"));
                } else {
                    ui.label(format!("{reference} {current}"));
                }
                match &available {
                    Some(available) => {
                        ui.label(format!("to {available}"));
                        if self.active_transaction.is_none()
                            && ui.add(crate::style::primary_button("Update")).clicked()
                        {
                            to_update = Some(reference.clone());
                        }
                    }
                    None => {
                        ui.weak("up to date");
                    }
                }
            });
        }
        if update_all {
            self.update(None);
        } else if let Some(reference) = to_update {
            self.update(Some(reference));
        }
    }

    /// A persistent warning, on every tab, while an installed package has
    /// been revoked by its publisher.
    fn show_revoked_warning(&mut self, ui: &mut egui::Ui) {
        let (_, revoked) = update_counts(&self.updates);
        if revoked == 0 {
            return;
        }
        let mut open_updates = false;
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(220, 60, 40, 50))
            .inner_margin(egui::Margin::same(8))
            .corner_radius(4)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{revoked} installed package(s) were revoked by their publisher. \
                         Update or remove them."
                    ));
                    if ui.button("Review").clicked() {
                        open_updates = true;
                    }
                });
            });
        ui.add_space(6.0);
        if open_updates {
            self.tab = Tab::Updates;
        }
    }
}

/// (packages with an update available, installed packages that were revoked).
fn update_counts(updates: &[Value]) -> (usize, usize) {
    let available = updates
        .iter()
        .filter(|item| item.get("available_version").is_some_and(|v| v.is_string()))
        .count();
    let revoked = updates
        .iter()
        .filter(|item| item.get("revoked").is_some_and(|v| v.is_string()))
        .count();
    (available, revoked)
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
            "alice@example.com (verified)"
        );

        let claim = serde_json::json!({
            "nip05_verified": false, "nip05": "linus@kernel.org", "claimed_name": "Linus"
        });
        let text = describe_publisher(Some(&claim), NPUB);
        assert!(text.contains("(unverified)") && text.contains("Linus"));
        assert!(!text.contains("(verified)"));

        // An unverified record must never earn the tick, even with a label.
        let forged = serde_json::json!({"nip05_verified": false, "label": "x@y.com"});
        assert!(!describe_publisher(Some(&forged), NPUB).contains("(verified)"));
    }

    #[test]
    fn listing_helpers_read_the_app_object_and_tolerate_its_absence() {
        let item = serde_json::json!({
            "name": "hello",
            "app": {
                "summary": "A friendly greeting",
                "categories": ["Utility", "Education"],
                "icon": {"url": "https://x/y", "sha256": "ab"}
            }
        });
        assert_eq!(app_text(&item, "summary"), Some("A friendly greeting"));
        assert_eq!(app_text(&item, "description"), None);
        assert_eq!(app_categories(&item), ["Utility", "Education"]);
        assert_eq!(icon_sha(&item), Some("ab"));

        let bare = serde_json::json!({"name": "hello"});
        assert_eq!(app_text(&bare, "summary"), None);
        assert!(app_categories(&bare).is_empty());
        assert_eq!(icon_sha(&bare), None);
        assert_eq!(
            app_text(&serde_json::json!({"app": {"summary": ""}}), "summary"),
            None
        );
    }

    #[test]
    fn shortcuts_map_to_actions_and_plain_keys_do_nothing() {
        use egui::Key as K;
        assert_eq!(
            shortcut_for(K::Num2, true),
            Some(Shortcut::GoTo(Tab::Browse))
        );
        assert_eq!(shortcut_for(K::F, true), Some(Shortcut::FocusSearch));
        assert_eq!(shortcut_for(K::F5, false), Some(Shortcut::Refresh));
        assert_eq!(shortcut_for(K::R, true), Some(Shortcut::Refresh));
        assert_eq!(shortcut_for(K::Escape, false), Some(Shortcut::CloseDetails));
        // Typing the letters or digits must never trigger anything.
        for key in [K::Num1, K::Num5, K::F, K::R] {
            assert_eq!(shortcut_for(key, false), None, "{key:?} without Ctrl");
        }
        assert_eq!(shortcut_for(K::Escape, true), None);
    }

    #[test]
    fn tabs_round_trip_through_their_saved_names() {
        for tab in Tab::ALL {
            assert_eq!(Tab::from_name(tab.name()), Some(tab));
        }
        assert_eq!(Tab::from_name("nonsense"), None);
    }

    #[test]
    fn update_counts_separate_available_updates_from_revoked_installs() {
        let updates = vec![
            serde_json::json!({"reference": "a/x", "available_version": "2.0.0"}),
            serde_json::json!({"reference": "a/y", "available_version": null}),
            serde_json::json!({"reference": "a/z", "available_version": "1.1.0", "revoked": "leaked key"}),
            serde_json::json!({"reference": "a/w"}),
        ];
        assert_eq!(update_counts(&updates), (2, 1));
        assert_eq!(update_counts(&[]), (0, 0));
    }

    #[test]
    fn history_is_described_in_plain_words() {
        assert_eq!(describe_age(1000, 1000), "just now");
        assert_eq!(describe_age(1000 + 60, 1000), "1 minute ago");
        assert_eq!(describe_age(1000 + 7200, 1000), "2 hours ago");
        assert_eq!(describe_age(1000 + 3 * 86_400, 1000), "3 days ago");
        let entry = serde_json::json!({
            "at": 1000, "action": "rollback", "reference": "npub1x/app",
            "from": {"version": "2.0.0"}, "to": {"version": "1.0.0"}
        });
        assert_eq!(
            history_line(&entry, 1000 + 120),
            "2 minutes ago: switched back npub1x/app 2.0.0 to 1.0.0"
        );
        let removal = serde_json::json!({"at": 0, "action": "remove", "reference": "npub1x/app", "from": {"version": "1.0.0"}});
        assert!(history_line(&removal, 0).ends_with("removed npub1x/app 1.0.0"));
    }

    #[test]
    fn browse_requests_filter_results_but_not_the_category_list() {
        let (search, categories) = browse_requests(&BrowseQuery {
            query: "edit".into(),
            relays: vec!["wss://r.example".into()],
            category: Some("Utility".into()),
            sort: BrowseSort::Recent.as_param(),
            limit: 48,
            only_trusted: true,
            refresh: false,
        });
        assert_eq!(search["query"], "edit");
        assert_eq!(search["category"], "Utility");
        assert_eq!(search["sort"], "recent");
        assert_eq!(search["limit"], 48);
        assert_eq!(search["host_only"], true);
        assert_eq!(search["include_untrusted"], true);
        assert_eq!(search["only_trusted"], true);
        assert_eq!(search["refresh"], false);
        // Counts cover the whole catalogue, under the same trust/host filters.
        assert!(categories.get("category").is_none() && categories.get("limit").is_none());
        assert_eq!(categories["only_trusted"], true);
        assert_eq!(categories["host_only"], true);

        let (everything, _) = browse_requests(&BrowseQuery {
            query: String::new(),
            relays: vec![],
            category: None,
            sort: BrowseSort::Name.as_param(),
            limit: 48,
            only_trusted: false,
            refresh: true,
        });
        assert!(everything.get("category").is_none());
        assert_eq!(
            (
                everything["query"].as_str(),
                everything["refresh"].as_bool()
            ),
            (Some(""), Some(true))
        );
        assert_eq!(everything["sort"], "name");
    }

    #[test]
    fn health_findings_are_counted_summarised_and_grouped() {
        let findings = vec![
            serde_json::json!({"level": "ok", "area": "Config", "title": "fine"}),
            serde_json::json!({"level": "warn", "area": "Store", "title": "hmm"}),
            serde_json::json!({"level": "fail", "area": "Relays", "title": "down"}),
            serde_json::json!({"level": "info", "area": "Config", "title": "note"}),
            serde_json::json!({"level": "warn", "area": "Relays", "title": "one slow"}),
        ];
        assert_eq!(health_counts(&findings), (1, 2));
        assert_eq!(health_counts(&[]), (0, 0));

        let groups = group_findings_by_area(&findings);
        let areas: Vec<&str> = groups.iter().map(|(area, _)| area.as_str()).collect();
        assert_eq!(
            areas,
            ["Config", "Store", "Relays"],
            "first-appearance order"
        );
        assert_eq!(groups[0].1.len(), 2, "same-area findings are together");

        assert_eq!(health_headline(0, 0), "Everything looks good.");
        assert!(health_headline(0, 2).contains("npack is working. 2 thing(s)"));
        assert!(health_headline(1, 0).starts_with("1 problem(s) stop npack working"));
        assert!(health_headline(1, 2).contains("plus 2 thing(s)"));
    }

    #[test]
    fn finding_markers_distinguish_problems_from_notes() {
        let marker = |level: &str| finding_marker(&serde_json::json!({"level": level})).0;
        assert_eq!(
            (
                marker("ok"),
                marker("warn"),
                marker("fail"),
                marker("info"),
                marker("??")
            ),
            ("", "Warning", "Problem", "", "")
        );
        assert_ne!(
            finding_marker(&serde_json::json!({"level": "fail"})).1,
            finding_marker(&serde_json::json!({"level": "ok"})).1
        );
    }

    #[test]
    fn stand_in_icons_are_stable_and_never_blank() {
        assert_eq!(
            tile_color_index("notepad-lite"),
            tile_color_index("notepad-lite")
        );
        assert!(
            ["a", "zebra", "", "\u{1F600}x"]
                .iter()
                .all(|n| tile_color_index(n) < TILE_COLORS.len())
        );
        assert_eq!(tile_letter("notepad-lite"), "N");
        assert_eq!(tile_letter("9lives"), "9");
        assert_eq!(tile_letter("--x"), "X", "skips punctuation");
        assert_eq!(tile_letter(""), "?");
        assert_eq!(
            tile_letter("\u{df}"),
            "SS",
            "uppercasing can make two letters"
        );
    }

    #[test]
    fn no_text_in_the_gui_needs_a_glyph_egui_does_not_ship() {
        // egui's built-in fonts lack ticks, crosses, arrows, dots and curly
        // quotes: they render as empty boxes. This guards every string the GUI
        // can show, so a symbol cannot sneak back in unseen.
        let sources = [include_str!("app.rs"), include_str!("friendly.rs")];
        let risky = [
            '\u{2713}', '\u{2717}', '\u{25CF}', '\u{2192}', '\u{26A0}', '\u{201C}', '\u{201D}',
            '\u{2018}', '\u{2019}',
        ];
        for (index, source) in sources.iter().enumerate() {
            // Skip this test's own list of the characters it forbids.
            let body: String = source
                .lines()
                .filter(|line| !line.contains("let risky"))
                .collect::<Vec<_>>()
                .join("\n");
            for glyph in risky {
                assert!(!body.contains(glyph), "file {index} contains {glyph:?}");
            }
        }
    }

    #[test]
    fn unknown_publishers_show_a_short_npub() {
        let text = describe_publisher(None, NPUB);
        assert!(text.starts_with("npub1") && text.contains('…') && text.len() < NPUB.len());
        let bare = serde_json::json!({"nip05_verified": false});
        assert_eq!(describe_publisher(Some(&bare), NPUB), text);
    }
}
