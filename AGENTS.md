# npack: guide for AI agents and contributors

npack is an independent package manager. Package metadata is published as signed
Nostr events, artifacts are stored on Blossom servers (content-addressed by
SHA-256), and the client does discovery, trust, dependency resolution,
verification, install, upgrade and removal itself, with no native package manager
required. This file is the canonical map; `CLAUDE.md` imports it. Read the
"Conventions" section below before changing behaviour, and `docs/` for depth.

## How it fits together

    publisher                          relays (Nostr)               Blossom servers
    ---------                          --------------               ---------------
    npack pack      -> package.npk  ----------------------------->   blob by sha256
    npack publish   -> kind 1063 (NIP-94 artifact event: url + x=sha256)  -> relays
                    -> kind 9900 (release event: name, version, os, arch, x, artifact,
                                  depends, requires/provides, post-install, kind, ...)
                    -> kind 9901 (revocation event, optional)

    client: npack update/resolve/unpack/search
      1. query relays (NIP-65 relay list for the pubkey, plus --relay/config)
      2. filter releases: signature, v1 tags, trusted publisher, not revoked,
         name, os/arch ("any" is a wildcard), semver requirement, lockfile
      3. fetch + verify the NIP-94 artifact event (same publisher, same x hash)
      4. download from artifact URLs / publisher Blossom servers (mirror fallback),
         accept only bytes whose SHA-256 equals the signed `x` tag
      5. resolve declared dependencies first (recursively), check conflicts and
         runtime capabilities
      6. install under an exclusive store lock with a rollback journal

Key facts to keep in mind:

- **Identity is `publisher/name/version`.** A publisher is a Nostr pubkey (hex or
  npub). Package names are not globally owned.
- **Nostr event metadata is authoritative; the `.npk` is just bytes** whose hash
  must match. The `.npk` is a deterministic tar compressed with zstd and may embed
  `.npack/manifest.json`.
- **Trust** is a client decision: trusted-publisher list, lockfile pins,
  revocations (kind 9901). Signatures and hashes prove who published which bytes,
  not that the package is safe.
- Protocol details: `docs/package-protocol-v1.md` (wire format, tags),
  `docs/package-event-fixtures.md`, `docs/release-key-model.md` and
  `docs/attestations.md` (both proposed, not implemented), `docs/package-setup.md`
  (install inputs).

## Workspace and code map

Cargo workspace (`edition = "2024"`): `npack-cli` (package `npack`, binary
`npack`, also the `npackd` daemon) and `npack-gui` (egui reference client).
`npack-cli/src/main.rs` is one large file (~9k lines, tests in `mod tests` at the
bottom). Search for the function names below instead of reading top to bottom.

| Area | Where (functions / types in `npack-cli/src/main.rs`) |
| --- | --- |
| CLI and config | `Cli`, `Command` enum, `main` dispatch; `Config` (`~/.config/npack/config.toml`: `[network].relays`, `[storage].blossom`, `[trust].publishers`, `[install].user`, `[identity].pubkey`) |
| Core types | `Manifest`, `ResolvedRelease`, `Dependency`, `InstalledPackage`, `Lockfile`/`LockedPackage`, `ForkOf`, `AppMetadata`, `InstallInput` |
| Packaging | `init_package`, `pack_npk`, `inspect_npk`, `load_embedded_manifest`, `extract_npk`, `unpack_command`, `appstream_xml` |
| Manifest / event validation | `validate_manifest_metadata`, `validate_artifact_kind`, `validate_fork_of`, `ensure_installable_kind`, `verify_manifest`, `validate_dependency_declarations`, `validate_capability_declarations` |
| Event building / signing | `sign_release_event`, `sign_artifact_event`, `sign_revocation_event`, `publish_release`, `verify_release_event`, `release_event_is_v1`, `validate_artifact_event` |
| Event parsing | `manifest_from_release` (tags -> `Manifest`), `parse_fork_of`, `tag_value`, `release_matches_target` |
| Discovery / resolve | `fetch_verified_release`, `fetch_verified_artifact_event`, `resolve_release`, `resolve_pinned_release` (`--release`), `finish_resolved_release`, `resolve_dependency_closure`, `resolve_command`, `collect_artifact_urls`, relay helpers (`add_user_relays`, `discover_blossom_servers`) |
| Download | `download_matching_blob`, `fetch_blob`, progress bars, local release/artifact cache (`cache_release`, ...) |
| Install / remove | `install_remote_package` (dependency-first), `install_with_capabilities_at` -> `install_locked`, `install_staged_npk`, `run_post_install`, `remove_package_at`, `ensure_dependencies_available`, `capability_satisfies` |
| Store safety | `acquire_store_lock`, `StoreTransaction`, `TransactionJournal`, `rollback_transaction` (stale-lock recovery) |
| Lockfiles | `write_lockfile`, `load_lockfile`, `validate_lockfile_graph`, `verify_locked_install` |
| Updates | `update_all_command`, `check_updates_command`, `install_remote_command` (CLI `update` / `install-ref`) |
| Catalogue | `Catalogue`, `fetch_catalogue_from_relays`, `refresh_command`, `search_matching_releases`, `info_command`, `catalogue_entries_for` |
| Daemon (npackd) | `run_daemon`, `dispatch_daemon_request`, `daemon_*` handlers, `TransactionEntry`/`TransactionControl`, `authorize_system_action` (PolicyKit via `pkcheck`, `SO_PEERCRED`) |
| Signing keys | `register_secret_key`, `resolve_secret_key` (OS keyring) |
| GUI | `npack-gui/src/client.rs` (JSON-RPC client), `app.rs` (egui) |

Other repository parts: `docs/` (protocol, roadmap, user guide, `npack.1`),
`packaging/` (`io.npack.policy`, `npackd-system.service`, and `packaging/nix/`:
`lib.nix`, `flake.nix`, `npack-nix-pin`, example recipe), `.github/workflows/`
(`ci.yml`, `release.yml`, `publish-linux-packages.yml`).

## Artifact kinds and Nix recipes

A release may carry an optional `kind` tag. Absent or `package` is an ordinary
installable package. Any other kind (currently `nix-recipe`) is never installed:
`install`/`update` refuse it before downloading, and it is consumed through
`npack resolve` (JSON on stdout). `fork-of` (publisher, name, release id) records
provenance only. `packaging/nix` turns resolved recipes into pinned, pure Nix
evaluations: `npack-nix-pin` writes a pins JSON, `lib.nix` `fromPins`/`fromPinsWith`
builds them in one scope. Workflow and trust notes: `docs/using-npack.md`
("Nix recipes"). `npack unpack --fork-as` scaffolds a fork.

## Building and testing

- `cargo fmt --all -- --check`, `cargo test --all-targets --all-features`,
  `cargo clippy --all-targets --all-features` (what CI runs; keep clippy clean).
- Unit tests build events and manifests in memory; no network needed.
- **End-to-end without public relays:** `nak serve --hostname 0.0.0.0 --port 10547
  --blossom` is an in-memory relay plus Blossom server (`go install
  github.com/fiatjaf/nak@latest`). Publish with `npack publish <manifest>
  --secret-key <hex> --relay ws://127.0.0.1:10547 --server http://127.0.0.1:10547`.
  Use a throwaway key. The relay forgets everything on restart.
- Isolate local state when experimenting: set `XDG_DATA_HOME` and
  `XDG_CONFIG_HOME` to scratch dirs (catalogue, caches and config live there), and
  pass `--store <dir>` for installs. Never test installs against `/` or your real
  `~/.local`.
- Nix integration needs a Nix install (a Debian QEMU/KVM VM with the Nix installer
  works; a guest reaches the host as `10.0.2.2`).
- The man page is generated from the clap definitions (`npack man`); `docs/npack.1`
  is only refreshed (and version-stamped) when a release is prepared, so help text
  lives in the `#[arg(help = ...)]` attributes and doc comments on `Command`.

## Gotchas

- `Manifest` and `ResolvedRelease` are built with struct literals in many tests;
  adding a field means updating them all (a `sed` after the `format:` line works).
- `SearchResult` is also the daemon's `Search` response, so changing it changes
  the daemon wire format (additive fields are fine).
- Release tags with a singleton rule are listed in two places
  (`manifest_from_release` and `release_event_is_v1`); keep both in sync when adding
  a tag, and also emit it in `sign_release_event` and compare it in
  `verify_release_event`.
- A dependency declared without a publisher resolves to any trusted publisher's
  package of that name; recipes and packages should name the publisher.
- Install and remove are serialized by `<store>/npack.lock`; a dead pid in the lock
  triggers rollback to the pre-transaction `installed.json`.
- `gh pr edit` can fail on this repository with a Projects (classic) GraphQL
  deprecation error; use `gh api -X PATCH repos/<owner>/<repo>/pulls/<n>` instead.
- Never execute an untrusted artifact to learn about it (ELF inspection is
  metadata-only), and never trust a mirror URL without checking the signed SHA-256.

## Current scope

- Cargo workspace: `npack-cli` (the CLI and npackd daemon, package name `npack`, binary `npack`) and `npack-gui` (a small reference `egui`/`eframe` desktop app). `cargo build`/`test`/`fmt`/`clippy` at the repo root cover both; `npack-cli`'s `default-run = "npack"` keeps bare `cargo run -- ...` at the root unambiguous.
- Rust CLI, terminal-first.
- Local package manifests and .npk artifacts are the first vertical slice.
- SHA-256 verification is mandatory before installation.
- System installation targets `/` with state in `/var/lib/npack`; `--user` targets `$HOME/.local`. Native package managers are not required.
- Relay-backed release discovery, NIP-65 relay-list discovery, Blossom retrieval with mirror fallback and caching, recursive dependency-first remote installation, lockfiles, conflicts, revocations, trusted publishers, declared dependency validation, and signed release-event generation are implemented with official crates. More advanced solving and publisher selection remain planned.

## Commands

    npack hash <artifact>
    npack verify <manifest>
    npack doctor [--user|--system] [--store <path>] [--offline] [--json]
    npack install <manifest> [--user|--system] [--store <path>] [--allow-capability <capability>]
    npack list [--user|--system] [--store <path>]
    npack release-event <manifest> --secret-key <hex-key>
    npack verify-event <event> <manifest>
    npack refresh [--relay <relay-url>] [--pubkey <hex>] [--store <path>]
    npack search <query> [--relay <relay-url>] [--refresh] [--no-cache] [--kind <kind>] [--category <name> | --uncategorised] [--sort name|recent] [--limit <n>] [--store <path>]
    npack categories [--refresh] [--all-platforms] [--relay <relay-url>] [--store <path>]
    npack info [<publisher>/]<name> [--trusted-publisher <hex>] [--store <path>]
    npack trust add <name@domain|npub|hex> [--yes] [--relay <url>]
    npack trust remove <name@domain|npub|hex>
    npack trust list [--names] [--relay <url>]
    npack fetch <sha256> --server <blossom-url> --output <path>
    npack update [<publisher>/]<name> --relay <relay-url> [--user|--system] [--store <path>] [--lockfile <path>] [--locked] [--allow-capability <capability>] [--check [--notify]]  (`install-ref` remains a compatibility alias)
    npack resolve [<publisher>/]<name> --relay <relay-url> [--requirement <semver>] [--lockfile <path>] [--locked] [--recursive] [--release <event-id>]
    npack unpack [<publisher>/]<name> --output <dir> [--release <event-id>] [--fork-as <publisher> [--name <name>]]
    npack init <dir> --name <name> --publisher <key> [--kind <kind>] [--fork-of <publisher>/<name>@<release-id>]
    npack pack <source-directory> --output <package.npk>
    npack remove <publisher>/<name> [--user|--system] [--store <path>]
    npack history [<publisher>/<name>] [--limit <n>] [--user|--system] [--store <path>]
    npack rollback <publisher>/<name> [--user|--system] [--store <path>] [--allow-capability <capability>]
    npack inspect <artifact>
    npack appstream <artifact> [--output <metainfo.xml>]
    npack daemon [--socket <path>] [--system]

## Conventions

- Keep package identity publisher-addressed: publisher/name/version.
- Never trust a mirror URL without checking the declared SHA-256.
- Prefer small, testable domain types over CLI-specific logic.
- Run cargo fmt --check and cargo test before committing.
- Provisional release events use kind 9900 and Nostr Schnorr signatures over the canonical event array.
- The canonical artifact format is .npk, a tar archive compressed with zstd; Nostr event metadata remains authoritative.
- ELF inspection is metadata-only; never execute an untrusted artifact to discover dependencies.
- ELF DT_NEEDED entries must be declared in runtime_requires when verifying an ELF artifact; symbol-version and capability resolution are future work.
- Runtime capability requirements are matched against installed package provides or host OS/architecture and standard shared-library capabilities; symbol-version and richer system capability providers are future work.
- Remote release selection must filter os and arch tags against the current host, accepting any as a wildcard.
- Runtime capabilities may be exact names or semver constraints matched against name@version provisions.
- Post-install hooks are declarative and signed; create-directory is package-local, while register-service is explicitly capability-gated.
- register-service is approved by the service-manager capability and installs a system or user systemd unit without enabling or starting it.
- `npack resolve` performs the same relay discovery, trust/semver/os-arch filtering, revocation check, and NIP-94 verification as `npack update`, but stops before downloading the artifact or installing anything, printing the resolved metadata as JSON for declarative package managers (e.g. Nix) to consume. `--recursive` walks and resolves the full declared dependency closure in one call, printing a JSON array instead of a single object.
- `npack update --check` reports available updates for one or all installed packages without downloading or installing anything, the `apt update` counterpart to `update`'s `apt upgrade`. `install-ref` is a compatibility alias for `update` kept for older scripts.
- A manifest's optional `app` object (`summary`, `description`, `homepage`, `license`, `categories`, `icon`, `screenshots`, `desktop_file`, `release_date`) carries desktop-store metadata; `icon` and `desktop_file` are package-relative paths validated to exist and, for `desktop_file`, to be a syntactically valid freedesktop.org Desktop Entry file with `Exec` required when `Type=Application`.
- `npack appstream` maps a manifest's `app` metadata to an AppStream `<component>` document per the freedesktop.org AppStream spec: `console-application` when there is no `desktop_file` (advertising `<provides><binary>`), `desktop-application` otherwise (advertising `<launchable type="desktop-id">`). Component IDs are namespaced `io.npack.<publisher>.<name>` since publishers are Nostr pubkeys, not domains.
- `npack daemon` runs npackd, a local JSON-RPC-over-Unix-socket service (newline-delimited JSON requests/responses) exposing `Search`, `GetPackage`, `ListInstalled`, `Install`, `Remove`, `Update`, `CheckUpdates`, `GetTransaction`, and `CancelTransaction` so a GUI store or other tool does not need to understand Nostr, Blossom, or `.npk` internals. It defaults to `$XDG_RUNTIME_DIR/npackd.sock` and handles connections concurrently via `tokio::spawn`. `Install`/`Update` run synchronously by default, or return `{"transaction_id": N}` immediately when called with `"async": true`, pollable via `GetTransaction`. `CancelTransaction` is cooperative: checked only between packages in a dependency graph or update loop, never mid-download or mid-install, so a cancelled transaction cannot leave the store half-installed. `GetTransaction` also reports a live progress snapshot (`connecting`/`resolving`/`downloading`/`updating`/`installed`, with the package being processed) while `running`, updated at the same package-level checkpoints; per-byte download progress remains future work.
- Every `install`/`remove` mutation of a package store (CLI or daemon) is serialized by an exclusive `npack.lock` file in the store root, so two operations against the same store never race on `installed.json` or a package directory; a concurrent attempt fails fast with "another npack operation is already in progress". Before acquiring the lock, npack checks whether the pid recorded in an existing lock is still alive (`/proc/<pid>`); a dead pid means the previous process was killed mid-transaction, so npack recovers automatically: it restores `installed.json` from the pre-transaction backup taken at the start of that transaction and, if the transaction had created a new package directory, removes it, before proceeding with the new operation. This bounds an interrupted install or remove to "state matches before the interrupted operation started", not a partially-applied one; `remove`'s file deletions themselves are not undone by this recovery, since that would require backing up every file before deleting it.
- CLI progress output (relay connect/resolve/query waits, artifact download) renders as an animated spinner or byte-progress bar on a real terminal and as today's plain `stderr` text otherwise (piped output, log files, `NO_COLOR`-style non-interactive contexts are unaffected); status words like `installed`/`up to date`/`available`/`[REVOKED]`/`(trusted)` on `stdout` are similarly colorized only when `stdout` is a terminal and `NO_COLOR` is unset. This is local rendering only -- it does not change the daemon's `GetTransaction` wire format, which still reports package-level (not per-byte) progress.
- `npack daemon --system` runs npackd-system: it must run as root (typically as a systemd system service, see `packaging/npackd-system.service`), binds a world-connectable socket (default `/run/npackd.sock`, mode 0666), and gates `Install`/`Remove`/`Update` from non-root peers through PolicyKit -- shelling out to `pkcheck --action-id <io.npack.install|remove|update> --process <peer-pid>` using the connecting peer's credentials read via the Unix socket's `SO_PEERCRED` (`UnixStream::peer_cred`), never trusting a client-supplied identity. The action definitions live in `packaging/io.npack.policy` (installed to `/usr/share/polkit-1/actions/` by system packaging, not by npack itself) and default to `auth_admin`. Root peers, and the plain `npack daemon` (npackd-user, unchanged) for a user's own `~/.local` store, are never gated. A peer whose credentials can't be read is denied rather than treated as authorized (fail closed).
- `npack refresh` fetches every release and revocation event from the configured/given relays and writes them to a durable local catalogue (`<store>/catalogue.json`, default the npack state dir), with no age-based expiry -- staleness is the user's call, made by running `refresh` again. `npack search` reads only this catalogue by default (no relay query per call); `--refresh` rebuilds it from relays first, and `--no-cache` does a one-off live relay query without touching the catalogue file. `npack info [<publisher>/]<name>` always reads the catalogue only (no relay escape hatch) and lists every known version/publisher/os/arch combination plus revocation and trusted-publisher status; it does not show AppStream fields, since release events don't carry them yet (publishing `app` metadata to Nostr tags is unstarted future work).
- Release `kind` (optional, e.g. `nix-recipe`) and `fork-of` (provenance only) tags: non-`package` kinds are never installed (`install`/`update` refuse before downloading); they are consumed via `npack resolve` and `packaging/nix` (`lib.nix`, `flake.nix`, `npack-nix-pin`). `npack unpack --fork-as` scaffolds a fork that inherits `kind` and records `fork_of`.
- Zero-config first run: empty `[network].relays` / `[storage].blossom` fall back to built-in public defaults (`DEFAULT_RELAYS`, `DEFAULT_BLOSSOM_SERVERS`; unverified community infrastructure, review before release) via `load_config_from`/`Config::apply_defaults`. No publisher is trusted by default. `Config.first_run` / `*_defaulted` are `#[serde(skip)]` flags reported by `GetConfig`.
- Daemon config is live: `DaemonState.config` is an `RwLock<Config>` (use `state.config()` for a snapshot). `GetConfig`, `ListTrustedPublishers`, `AddTrustedPublisher`, `RemoveTrustedPublisher` and `SaveConfig` read/write it, persisting through `update_config_file` (read-modify-write of a generic TOML table: unknown keys survive, comments do not, an unparsable file is never overwritten). The three mutating ones are gated on the system daemon by the `io.npack.configure` polkit action (`packaging/io.npack.policy`).
- Trust semantics: the CLI filter is unchanged (empty list = accept any publisher). `Search`/`GetPackage` take `include_untrusted` and always add `trust` (`trusted`/`unknown`, where an empty list means nobody is trusted yet) and `npub`; `Install` takes `trust_once[]`. Daemon error responses and failed transactions carry an additive `error_code` (`daemon_error_code`); add new categories there, in `docs/using-npack.md`, and in `npack-gui/src/friendly.rs`.
- Publisher names are NIP-05 (`parse_nip05`, `fetch_nip05_pubkey_at`, `resolve_publisher_input`, `build_identity`): HTTPS-only, no redirects, size/time-capped, no IP/port/local domains. A name is a display aid only; the key is stored. Only a name the domain maps back to the same key becomes `label` (verified); profile-claimed names stay `claimed_name`/`nip05` and the GUI must mark them "(unverified)" (`describe_publisher`). `ResolvePublisher`/`GetPublisherProfiles` are read-only and ungated; `AddTrustedPublisher` also accepts `name@domain`. Identities are cached in memory for an hour (`DaemonState.identities`).
- `npack trust add|remove|list` (`trust_command`) edits only `[trust].publishers` via the helpers shared with the daemon (`trust_list_with`, `trust_list_without`, `persist_trust_list`). `add` always shows the resolved npub and needs `confirmation_decision` consent (`--yes`, or a terminal answer; non-tty without `--yes` is refused). The daemon re-reads `config.toml` when its mtime changes (`DaemonState::reload_config_if_changed`) and calls `note_config_written` after its own writes, so CLI and GUI edits never clobber each other; a file that stops parsing keeps the previous in-memory config. Both CLI and daemon mutations treat "already trusted" as a no-op that does not touch the file.
- Store-listing metadata is published as additive release tags (`summary`, `description`, `homepage`, `license`, `release-date`, `icon <url> <sha256>`, repeatable `category`/`screenshot`), no protocol-version bump. Everything funnels through `AppListing::from_app` (clean + bound + drop-invalid), so publish (`sign_release_event` -> `AppListing::tags`), read (`app_metadata_from_event`, lenient: bad tags are dropped, never fatal) and `verify_release_event` agree. The singleton ones are in both singleton lists (`manifest_from_release`, `release_event_is_v1`). `validate_app_metadata` is the strict publish-time check (http(s)-only URLs, length/count caps). `npack publish` extracts `app.icon` from the `.npk` (`read_npk_file`), sniffs PNG/SVG (`icon_media_type`) and uploads it as its own Blossom blob (`publish_icon`, `upload_to_blossom`); an icon failure warns and publishes without it. `Search` results and `GetPackage` carry the sanitised listing as `app`, and search also matches summary/category.
- `GetIcon` (`get_icon_at`, `fetch_icon_bytes`, `verify_icon_bytes`) serves a catalogue release's icon as base64: URL comes only from the signed event, https + public DNS name only, no redirects, 256 KiB cap, signed-hash and PNG/SVG-by-bytes check, cached by hash under `<store>/icons` and re-verified on every serve. `npack-gui` loads icons in the background (`request_icons`, at most 24 per call), decodes them with `egui_extras` image loaders, and shows summary/categories in search rows and a structured details pane; it never fetches screenshots or homepages itself (the homepage is a click-to-open link).
- Update checks: `check_updates_core` (shared by the daemon's `CheckUpdates` and `npack update --check --notify`) returns `UpdateStatus` including `revoked`, set by `installed_revocation_reason` (a valid v1 revocation signed by the package's own publisher for that exact name/version/hash, from `fetch_revocation_events`, best effort). `--notify` (`notify_updates_command`) shells out to `notify-send` (no D-Bus client dependency, as with `pkcheck`) only for items not in `<store>/update-notified.txt` (`update_notice`: pure and tested); a revocation is critical urgency. Notification text is escaped (`escape_notification_markup`). `packaging/npack-update-check.{service,timer}` are user units. The GUI checks once at start, shows `Updates (N)`, a warning strip while anything is revoked, and per-package / "Update all" buttons (async `Update` transactions).
- History and rollback: installs/removals append to `<store>/history.jsonl` (`append_history`, best effort, never fails an install; written by `install_with_history_at`, which `install_with_capabilities_at` wraps, and `remove_package_at`). `rollback_package_at` is NOT a restore mechanism: `plan_rollback` verifies that history names a previous version (`rollback_target`), that a file in `packages/<pub>/<name>/<version>/` still hashes to the recorded SHA-256 (the stored archive is named `artifact`, not `*.npk`, so it is found by hash), and that the signed release event in the metadata cache is present (`load_cached_release`); then it reinstalls via the normal transactional `install_with_history_at` from a private staging copy (the install path copies the archive into the same directory it lives in), refusing if an installed dependent's requirement would break. Rollback is offline and does not re-check revocation. Daemon: `GetHistory`, `ListRollbacks` (only rollbacks that would pass `plan_rollback`), `Rollback` (polkit `io.npack.update`). GUI: "Switch back to X" on the Installed tab and a History list.
- Browsing: `SearchResult` also carries `published_at` and `compatible` (os/arch fit, via `release_matches_target`). `apply_search_options` (category, sort name/recent, offset, limit) is pure and shared by `npack search`, the daemon's `Search`, and tests; `only_trusted`/`host_only` are applied first in `daemon_search_results`, which `ListCategories` reuses so counts match what the browse view shows. `category_counts` merges spelling variants and picks the display spelling deterministically (most common, then capitalised). An empty query matches everything. The GUI "Browse" tab (`browse`, `browse_requests`) has a category sidebar, Recently-updated/Name sort, "Only publishers I trust", and "Show more" paging, and on `no_catalogue` fetches the catalogue itself exactly once.
- `npack doctor` (`doctor_command`) diagnoses without changing anything: each area is a small function returning `Finding`s (`doctor_config`, `doctor_store`, `doctor_installed`, `doctor_catalogue`, `doctor_tools`, `doctor_daemon`, `doctor_network`) so it is unit-tested directly; `render_doctor_report` / `--json` only present them. Levels are Ok/Info/Warn/Fail and only Fail makes the exit status 1; optional helpers and a missing daemon are never worse than Info, one dead relay is a Warn and no working relay is a Fail. It must stay read-only: it never takes the store lock (that would trigger recovery) and the only write is a self-deleted probe file in an existing store. `main` parses the command line before loading the config and lets only `Doctor` continue past a config that fails to parse, so it can report it (every other command still stops, and `--help`/`--version` now work even then). Relays are checked with the nostr-sdk status poll, download servers with a plain GET (any HTTP answer counts, Blossom often 404s `/`); timeouts are injected so tests are fast.
- Health in the GUI: `RunDoctor` (`daemon_run_doctor`) returns `{healthy, findings}` from the same `doctor_findings` the CLI uses (both build a `DoctorPlan`; the daemon sets `probe_service: false` so it reports "This service is running" instead of connecting to its own socket). The sync checks run in `spawn_blocking`. It is read-only and ungated, so it is serialised by `DaemonState.doctor_gate` and a result is reused for `DOCTOR_CACHE_SECS` (3s) when the request (`offline`, `user`, `store`) matches. The GUI's Health tab (`run_health_check`, `show_health_tab`) runs it at start, badges the tab, and shows a strip on every tab only for `fail` findings; a failure to run it is shown inside the tab, not as a banner.
- Packaging and releases: `packaging/build-linux-packages.sh` builds `npack` (CLI + service, man page, systemd units, PolicyKit policy; no graphics libraries), `npack-gui` and the empty `npack-desktop` meta-package as `.deb` and `.rpm`, and `packaging/build-npk.sh` builds the `.npk` archives and manifests published to Nostr (the GUI's carries a full store listing). Both release workflows and CI call these scripts, so run them locally with debug binaries (`packaging/build-linux-packages.sh 0.0.0 target/debug /tmp/out`) after changing packaging; CI does this on every PR. The GUI's runtime libraries are loaded with dlopen, so they are declared by hand in both scripts (found with `LD_DEBUG=libs`); keep the two lists in step. `release.yml` refuses a tag that does not match the `Cargo.toml` versions of `npack` and `npack-gui` (keep the two crates on the same version), builds everything, creates the GitHub Release and then publishes every manifest to Nostr (the `release` environment, `NOSTR_SECRET_KEY`). `desktop/` holds the launcher entry, icon and AppStream file (`@VERSION@`/`@DATE@` are filled in at build time).
- The GUI starts the daemon itself (`DaemonClient::ensure_daemon`, only when nothing listens, in its own process group), `EventSender` wakes egui when a background result arrives (egui redraws only on input), and no GUI string may contain a tick, cross, arrow, dot or curly quote because egui's default fonts lack them (a test enforces it; draw shapes or spell it out). npackd-user defaults to the user's own prefix (`DaemonState::config`).
- `npack-gui` first-run welcome, Settings tab (trusted publishers, relays, download servers), unknown-publisher confirmation dialog and plain-language error banner are driven entirely by those daemon methods.
- `npack-gui` is a plain client of npackd's documented JSON-RPC-over-Unix-socket protocol (`npack-gui/src/client.rs`), with no dependency on `npack-cli`'s internals -- keep it that way, since proving `npackd` is frontend-independent is the entire point of Phase 5. Every daemon call runs on a background thread and reports back through an `mpsc` channel drained each frame, so a slow relay/Blossom call never freezes the window; `Install` uses `"async": true` and polls `GetTransaction` for progress rather than blocking on the synchronous call.
- Releasing: bump both `Cargo.toml` versions and `Cargo.lock`, merge, then run the `Create release` workflow (`.github/workflows/create-release.yml`) on the default branch with the version and a summary. It runs `packaging/check-release-version.sh`, creates the annotated `vX.Y.Z` tag with the workflow token and dispatches `release.yml` on it (a token-pushed tag does not trigger workflows by itself). A manual `release.yml` run on a branch is a rehearsal: no GitHub Release, no Nostr. See `docs/github-actions.md`.
- GUI look and behaviour: `npack-gui/src/style.rs` sets the type scale, spacing and accent for both themes (egui follows the system light/dark setting), adds system fonts (`FALLBACK_FONT_FILES`, found by `find_fallback_fonts`, nothing bundled) as fallbacks, and has `primary_button` for the main action; the accent must keep WCAG AA contrast with white text (a test checks it). egui has no bidi, so right-to-left text is shown in the wrong order, and the default-font glyph ban still stands since a fallback font is not guaranteed. `remember.rs` keeps the tab and window size in `gui-state.json` (lenient read, size clamped). Keyboard shortcuts live in `shortcut_for`/`handle_shortcuts` (Ctrl+1..5, Ctrl+F, F5/Ctrl+R, Escape), and Browse searches `SEARCH_DELAY` after typing stops. Under Xvfb with no window manager the window never gets keyboard focus: run `xdotool windowfocus` and click it before sending keys.
- Listing additions after 0.5.0: `release-notes`, `support` (http(s) link), `download-size` and `installed-size` are singleton listing tags in `AppListing` (also in `APP_LISTING_TAGS` and both singleton lists). The two sizes are written only by `npack publish` (`installed_size_of`, artifact length) and are ignored when comparing an event with a pre-publish manifest (`AppListing::without_publish_facts`). `listing_warnings` makes `publish` say when a release has no summary or categories. `newest_per_release` keeps one event per publisher/name/version/os/arch, so republishing fixes a listing. `SearchResult.details` (`ReleaseDetails`, from `release_details`) carries repo, commit, depends, requires and post-install actions for a "what installing does" view. `SearchOptions.uncategorised` / `uncategorised_count` and the last `ListCategories` entry (`"uncategorised": true`) make listing-less packages browsable.
- GUI listing views: Browse rows and the details pane read sizes, release notes, support link and `details` (post-install actions via `install_effects`, requirements, `source_line`) from the daemon's `app`/`details` objects; the install confirmation shows the same effects from `GetPackage` (`ResolvedRelease.details`). `CheckUpdates` items carry `release_notes` and `download_size` for the available version. The sidebar's `CategoryEntry` marks the daemon's `uncategorised` entry, which is searched with `"uncategorised": true` (only when no category is chosen).
- GUI install scope: `InstallScope` (`user`, the default, or `system`) picks which npackd the GUI talks to (`InstallScope::socket`; the system one is `/run/npackd.sock`, `client::SYSTEM_SOCKET`). The "Everyone on this computer" option in Settings only exists while `system_available` (checked off-thread every `SYSTEM_CHECK_EVERY`, which also schedules the repaint, since egui redraws only on input) or while it is already selected. `DaemonClient::ensure_daemon` never starts a daemon for the system socket. Switching (`set_scope`) clears everything shown and reconnects, skips the first-run welcome (a person's own prefix has a first run, the system one does not), and `Event::SystemService(false)` while in system scope drops back to user. The choice is remembered in `gui-state.json` (`scope`). The real PolicyKit prompts cannot be exercised under Xvfb as root; test the switch, detection and fallback.
- Hard-coded path checks: `npack-cli/src/pathcheck.rs` (`check`, over `Entry` lists from `load_dir` or `load_npk`) warns from `npack pack` (`pack_with_path_checks`; `--strict` fails before writing, `--allow-path` and `.npack/path-allow.txt` allow) and `npack publish`. Rules: an absolute path naming a file the package itself installs (after dropping `usr/` or `usr/local/`; top-level folders like `/usr/bin` never count), build-machine paths (the pack directory, `/home/<x>`, ... in text only; binaries only for the pack directory), ELF runpaths outside the standard library folders, absolute symlinks. Static only: never run package contents. `packaging/build-npk.sh` packs our own packages with `--strict`.
- Install-time placeholder: `{{prefix}}` (`npack-cli/src/templates.rs`) is expanded, only in the files listed in the package's `.npack/templates.txt`, by `templates::apply_to_staging`, called from `install_locked` on the staging copy before `install_staged_npk` (so the stored archive stays pristine and rollback re-expands it). The prefix has no trailing slash and is empty for an install at `/`. Nothing else is expanded, and unlisted files are never touched. `templates::validate_entries` runs at pack time; `pathcheck` reports `UnlistedPlaceholder` and `UnusedTemplate`. A new token would need documenting in `docs/using-npack.md` and a decision on whether it can leak anything about the machine.
- Catalogue freshness: `CatalogueStatus` (read-only) and `RefreshCatalogue` (`daemon_catalogue_status`, `daemon_refresh_catalogue`; one download at a time through `DaemonState.refresh_gate`, skipped when the catalogue is under `CATALOGUE_FRESH_SECS` old; `CATALOGUE_STALE_SECS` = 6 h decides `stale`). `save_refreshed_catalogue` refuses to replace a non-empty catalogue with an empty download, because unreachable relays return no events instead of failing; every refresh path uses it. The GUI (`check_catalogue`, `refresh_catalogue`, `should_refresh_catalogue`, `catalogue_line`) checks at start and every `CATALOGUE_CHECK_EVERY`, refreshes in the background when stale or on the first start after an update (`Remembered.version`), keeps the old list on screen, and shows a note rather than an error banner on failure. `npack search` prints the age with `human_age`.
