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
  `docs/package-event-fixtures.md`, `docs/release-key-model.md` (proposed, not
  implemented), `docs/package-setup.md` (install inputs).

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
    npack install <manifest> [--user|--system] [--store <path>] [--allow-capability <capability>]
    npack list [--user|--system] [--store <path>]
    npack release-event <manifest> --secret-key <hex-key>
    npack verify-event <event> <manifest>
    npack refresh [--relay <relay-url>] [--pubkey <hex>] [--store <path>]
    npack search <query> [--relay <relay-url>] [--refresh] [--no-cache] [--kind <kind>] [--store <path>]
    npack info [<publisher>/]<name> [--trusted-publisher <hex>] [--store <path>]
    npack fetch <sha256> --server <blossom-url> --output <path>
    npack update [<publisher>/]<name> --relay <relay-url> [--user|--system] [--store <path>] [--lockfile <path>] [--locked] [--allow-capability <capability>] [--check]  (`install-ref` remains a compatibility alias)
    npack resolve [<publisher>/]<name> --relay <relay-url> [--requirement <semver>] [--lockfile <path>] [--locked] [--recursive] [--release <event-id>]
    npack unpack [<publisher>/]<name> --output <dir> [--release <event-id>] [--fork-as <publisher> [--name <name>]]
    npack init <dir> --name <name> --publisher <key> [--kind <kind>] [--fork-of <publisher>/<name>@<release-id>]
    npack pack <source-directory> --output <package.npk>
    npack remove <publisher>/<name> [--user|--system] [--store <path>]
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
- `npack-gui` is a plain client of npackd's documented JSON-RPC-over-Unix-socket protocol (`npack-gui/src/client.rs`), with no dependency on `npack-cli`'s internals -- keep it that way, since proving `npackd` is frontend-independent is the entire point of Phase 5. Every daemon call runs on a background thread and reports back through an `mpsc` channel drained each frame, so a slow relay/Blossom call never freezes the window; `Install` uses `"async": true` and polls `GetTransaction` for progress rather than blocking on the synchronous call.
