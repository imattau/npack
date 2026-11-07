//! Turns npackd errors into wording a non-technical person can act on.
//!
//! npackd attaches a stable `error_code` to every failed response and failed
//! transaction (see "Error codes" in docs/using-npack.md). The GUI keys its
//! messages off that code and keeps the daemon's raw text as an expandable
//! detail, so wording can improve without touching the daemon.

use serde_json::Value;

use crate::client::DaemonError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiError {
    /// npackd's `error_code`, or `daemon_unreachable` when the socket could
    /// not be reached at all.
    pub code: Option<String>,
    /// The daemon's original message, shown under "Details".
    pub detail: String,
}

impl UiError {
    pub fn from_anyhow(error: &anyhow::Error) -> Self {
        if let Some(daemon) = error.downcast_ref::<DaemonError>() {
            return Self {
                code: daemon.code.clone(),
                detail: daemon.message.clone(),
            };
        }
        let detail = error.to_string();
        let code = detail
            .contains("connecting to npackd")
            .then(|| "daemon_unreachable".to_owned());
        Self { code, detail }
    }

    /// A failed `GetTransaction` status: `{"status":"failed","error":..,"error_code":..}`.
    pub fn from_failed_status(status: &Value) -> Option<Self> {
        if status.get("status").and_then(Value::as_str) != Some("failed") {
            return None;
        }
        Some(Self {
            code: status
                .get("error_code")
                .and_then(Value::as_str)
                .map(str::to_owned),
            detail: status
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned(),
        })
    }

    /// One plain sentence saying what happened and what to do next.
    pub fn headline(&self) -> &'static str {
        match self.code.as_deref() {
            Some("daemon_unreachable") => {
                "npack's background service isn't running. Start it with `npack daemon`, then press Reconnect."
            }
            Some("no_relays") => {
                "No relays are set up. Open Settings to add some or restore the recommended ones."
            }
            Some("no_blossom_servers") => {
                "No download servers are set up. Open Settings to add some or restore the recommended ones."
            }
            Some("relay_unreachable") => {
                "Couldn't reach any relay. Check your internet connection, or try different relays in Settings."
            }
            Some("store_busy") => {
                "Another install or removal is already running. Wait for it to finish, then try again."
            }
            Some("publisher_untrusted") => {
                "You haven't trusted this publisher yet. Trust them first to continue."
            }
            Some("not_authorized") => {
                "That needs administrator approval, and it wasn't given. Nothing was changed."
            }
            Some("download_failed") => {
                "The download failed or didn't match its signed checksum. Nothing was installed."
            }
            Some("cancelled") => "Cancelled. Nothing was changed.",
            Some("not_found") => {
                "Couldn't find that package or release. Try refreshing the catalogue."
            }
            Some("unknown_method") => {
                "This app is newer than the npack background service. Update npack and restart the service."
            }
            Some("no_catalogue") => {
                "The software catalogue hasn't been downloaded yet. Press \"Refresh catalogue\" to fetch it."
            }
            Some("no_rollback") => "There's no earlier version of this package to switch back to.",
            Some("rollback_blocked") => {
                "Couldn't switch back safely. The saved copy is missing or has changed, or something installed needs the newer version. Nothing was changed."
            }
            Some("not_a_package") => {
                "That file isn't an npack package. It needs to be a .npk file made with npack."
            }
            Some("unsupported_here") => {
                "Installing a file isn't available for everyone yet. Switch to \"Just me\" in Settings and try again."
            }
            Some("nip05_failed") => {
                "Couldn't verify that name. Check the spelling, or paste the publisher's npub instead."
            }
            Some("invalid_input") => {
                "That doesn't look right. Check what you entered and try again (see Details)."
            }
            Some("invalid_params" | "invalid_request") => {
                "The app and the background service disagree about this request. Update both to the same version."
            }
            _ => "Something went wrong.",
        }
    }

    /// Whether the fix is in the Settings tab, so the UI can offer a shortcut.
    pub fn suggests_settings(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some("no_relays" | "no_blossom_servers" | "relay_unreachable")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_errors_map_to_actionable_wording() {
        let error = UiError::from_anyhow(&anyhow::Error::new(DaemonError {
            code: Some("relay_unreachable".into()),
            message: "timed out connecting to configured Nostr relays".into(),
        }));
        assert!(error.headline().contains("Couldn't reach any relay"));
        assert!(error.suggests_settings());
        assert!(error.detail.contains("timed out"));
    }

    #[test]
    fn a_missing_socket_is_reported_as_the_service_not_running() {
        let error = UiError::from_anyhow(&anyhow::anyhow!(
            "connecting to npackd at /run/user/1/npackd.sock: refused (is `npack daemon` running?)"
        ));
        assert_eq!(error.code.as_deref(), Some("daemon_unreachable"));
        assert!(error.headline().contains("isn't running"));
        assert!(!error.suggests_settings());
    }

    #[test]
    fn a_missing_catalogue_points_at_refreshing_it() {
        let error = UiError {
            code: Some("no_catalogue".into()),
            detail: "no local catalogue found".into(),
        };
        assert!(error.headline().contains("Refresh catalogue"));
    }

    #[test]
    fn name_lookup_failures_have_their_own_advice() {
        let error = UiError {
            code: Some("nip05_failed".into()),
            detail: "NIP-05 lookup failed for a@b.com".into(),
        };
        assert!(error.headline().contains("verify that name"));
    }

    #[test]
    fn unknown_or_missing_codes_fall_back_without_hiding_the_detail() {
        let error = UiError::from_anyhow(&anyhow::anyhow!("boom"));
        assert_eq!(error.headline(), "Something went wrong.");
        assert_eq!(error.detail, "boom");
    }

    #[test]
    fn failed_transaction_statuses_carry_their_code() {
        let status = serde_json::json!({
            "status": "failed", "error": "another npack operation is already in progress",
            "error_code": "store_busy"
        });
        let error = UiError::from_failed_status(&status).unwrap();
        assert!(error.headline().contains("already running"));
        assert!(UiError::from_failed_status(&serde_json::json!({"status": "succeeded"})).is_none());
    }
}
