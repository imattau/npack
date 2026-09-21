# npack

npack is an independent package manager whose registry metadata will be published on Nostr and whose artifacts may be stored on Blossom. It owns package discovery, trust, dependency resolution, verification, installation, upgrades, and removal.

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
    npack search <query> [--relay <relay-url>] [--refresh] [--no-cache] [--store <path>]
    npack info [<publisher>/]<name> [--trusted-publisher <hex>] [--store <path>]
    npack fetch <sha256> --server <blossom-url> --output <path>
    npack install-ref [<publisher>/]<name> --relay <relay-url> [--user|--system] [--store <path>] [--lockfile <path>] [--locked] [--allow-capability <capability>] [--check]
    npack resolve [<publisher>/]<name> --relay <relay-url> [--requirement <semver>] [--lockfile <path>] [--locked] [--recursive]
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
- `npack resolve` performs the same relay discovery, trust/semver/os-arch filtering, revocation check, and NIP-94 verification as `npack install-ref`, but stops before downloading the artifact or installing anything, printing the resolved metadata as JSON for declarative package managers (e.g. Nix) to consume. `--recursive` walks and resolves the full declared dependency closure in one call, printing a JSON array instead of a single object.
- `npack update --check` (alias of `install-ref --check`) reports available updates for one or all installed packages without downloading or installing anything, the `apt update` counterpart to `update`'s `apt upgrade`.
- A manifest's optional `app` object (`summary`, `description`, `homepage`, `license`, `categories`, `icon`, `screenshots`, `desktop_file`, `release_date`) carries desktop-store metadata; `icon` and `desktop_file` are package-relative paths validated to exist and, for `desktop_file`, to be a syntactically valid freedesktop.org Desktop Entry file with `Exec` required when `Type=Application`.
- `npack appstream` maps a manifest's `app` metadata to an AppStream `<component>` document per the freedesktop.org AppStream spec: `console-application` when there is no `desktop_file` (advertising `<provides><binary>`), `desktop-application` otherwise (advertising `<launchable type="desktop-id">`). Component IDs are namespaced `io.npack.<publisher>.<name>` since publishers are Nostr pubkeys, not domains.
- `npack daemon` runs npackd, a local JSON-RPC-over-Unix-socket service (newline-delimited JSON requests/responses) exposing `Search`, `GetPackage`, `ListInstalled`, `Install`, `Remove`, `Update`, `CheckUpdates`, `GetTransaction`, and `CancelTransaction` so a GUI store or other tool does not need to understand Nostr, Blossom, or `.npk` internals. It defaults to `$XDG_RUNTIME_DIR/npackd.sock` and handles connections concurrently via `tokio::spawn`. `Install`/`Update` run synchronously by default, or return `{"transaction_id": N}` immediately when called with `"async": true`, pollable via `GetTransaction`. `CancelTransaction` is cooperative: checked only between packages in a dependency graph or update loop, never mid-download or mid-install, so a cancelled transaction cannot leave the store half-installed. `GetTransaction` also reports a live progress snapshot (`connecting`/`resolving`/`downloading`/`updating`/`installed`, with the package being processed) while `running`, updated at the same package-level checkpoints; per-byte download progress remains future work.
- Every `install`/`remove` mutation of a package store (CLI or daemon) is serialized by an exclusive `npack.lock` file in the store root, so two operations against the same store never race on `installed.json` or a package directory; a concurrent attempt fails fast with "another npack operation is already in progress". Before acquiring the lock, npack checks whether the pid recorded in an existing lock is still alive (`/proc/<pid>`); a dead pid means the previous process was killed mid-transaction, so npack recovers automatically: it restores `installed.json` from the pre-transaction backup taken at the start of that transaction and, if the transaction had created a new package directory, removes it, before proceeding with the new operation. This bounds an interrupted install or remove to "state matches before the interrupted operation started", not a partially-applied one; `remove`'s file deletions themselves are not undone by this recovery, since that would require backing up every file before deleting it.
- `npack daemon --system` runs npackd-system: it must run as root (typically as a systemd system service, see `packaging/npackd-system.service`), binds a world-connectable socket (default `/run/npackd.sock`, mode 0666), and gates `Install`/`Remove`/`Update` from non-root peers through PolicyKit -- shelling out to `pkcheck --action-id <io.npack.install|remove|update> --process <peer-pid>` using the connecting peer's credentials read via the Unix socket's `SO_PEERCRED` (`UnixStream::peer_cred`), never trusting a client-supplied identity. The action definitions live in `packaging/io.npack.policy` (installed to `/usr/share/polkit-1/actions/` by system packaging, not by npack itself) and default to `auth_admin`. Root peers, and the plain `npack daemon` (npackd-user, unchanged) for a user's own `~/.local` store, are never gated. A peer whose credentials can't be read is denied rather than treated as authorized (fail closed).
- `npack refresh` fetches every release and revocation event from the configured/given relays and writes them to a durable local catalogue (`<store>/catalogue.json`, default the npack state dir), with no age-based expiry -- staleness is the user's call, made by running `refresh` again. `npack search` reads only this catalogue by default (no relay query per call); `--refresh` rebuilds it from relays first, and `--no-cache` does a one-off live relay query without touching the catalogue file. `npack info [<publisher>/]<name>` always reads the catalogue only (no relay escape hatch) and lists every known version/publisher/os/arch combination plus revocation and trusted-publisher status; it does not show AppStream fields, since release events don't carry them yet (publishing `app` metadata to Nostr tags is unstarted future work).
- `npack-gui` is a plain client of npackd's documented JSON-RPC-over-Unix-socket protocol (`npack-gui/src/client.rs`), with no dependency on `npack-cli`'s internals -- keep it that way, since proving `npackd` is frontend-independent is the entire point of Phase 5. Every daemon call runs on a background thread and reports back through an `mpsc` channel drained each frame, so a slow relay/Blossom call never freezes the window; `Install` uses `"async": true` and polls `GetTransaction` for progress rather than blocking on the synchronous call.
