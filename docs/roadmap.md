# npack development roadmap

The near-term objective is to prove npack as a frontend-independent package
service before committing to any particular desktop store. The first major
milestone is Phases 1–5: metadata compatibility, a stable service layer,
security and privilege separation, a local catalogue, and a small reference
GUI.

## Phase 1: Package metadata compatibility (done)

Add first-class AppStream support:

- Map `.npk` metadata to AppStream fields via a manifest's optional `app`
  object.
- Support icons, screenshots, categories, homepage, licence, summary, and
  description.
- Add desktop-file validation: `npack pack` requires a declared
  `app.desktop_file` to exist and satisfy the freedesktop.org Desktop Entry
  spec (`Type`, `Name`, and `Exec` when `Type=Application`).
- Add `npack appstream <package>` output, rendering a `console-application` or
  `desktop-application` AppStream `<component>` document depending on whether
  `app.desktop_file` is set.
- Validate against AppStream tooling in CI: the `rust` CI job installs
  `appstreamcli` and runs `appstreamcli validate --no-net` against a generated
  sample document.

Goal: an npack package can describe itself in the language already understood
by Linux application stores.

## Phase 2: Stable service layer (done)

Introduce `npackd` as the common local backend for the CLI, GUI store plugins,
and other tools:

```text
CLI / GUI plugins / other tools
              │
              ▼
            npackd
              ├─ package state
              ├─ Nostr discovery
              ├─ Blossom fetch
              ├─ dependency resolution
              ├─ verification
              └─ install/update/remove
```

Keep the API local and narrow. D-Bus is the leading Linux-native option; a
Unix socket API remains an alternative.

Shipped: `npack daemon` runs npackd as a JSON-RPC-over-Unix-socket service
(newline-delimited JSON, defaulting to `$XDG_RUNTIME_DIR/npackd.sock`)
exposing:

```text
Search()          GetPackage()
ListInstalled()   Install()
Remove()          Update()
CheckUpdates()    GetTransaction()
CancelTransaction()
```

Chose a Unix socket over D-Bus for this first slice: no new system dependency
or session/system bus requirement, so it behaves the same in minimal and
containerized environments. A D-Bus adapter in front of the same handlers
remains possible later if a desktop-store integration phase needs it.

npackd handles connections concurrently (each accepted connection is
`tokio::spawn`ed), which required making the recursive dependency-install
future `Send`.

`Install`/`Update` run synchronously by default; passing `"async": true`
returns `{"transaction_id": N}` immediately, pollable via `GetTransaction`.
`CancelTransaction` is cooperative rather than raw task abortion: a shared
cancellation flag is checked only between packages (before starting the next
package in a dependency graph, or the next package in an `Update` loop),
never mid-download or mid-install of a package already in progress -- a
cancelled transaction cannot leave the store half-installed.

`GetTransaction` also reports a live progress snapshot (`connecting`,
`resolving`/`downloading`/`updating` a named package, `installed`) while a
transaction is `running`, updated at the same package-level checkpoints as
cancellation -- not per-byte download progress, but enough for a GUI to show
"resolving foo...", "downloading bar (3 mirrors)...", etc. without polling
`ps` or scraping stderr.

Remaining work for a future phase: per-byte/per-file download progress
within a single package's fetch, if a GUI needs a progress bar rather than a
status line.

## Phase 3: Security and privilege separation (in progress)

Installer-neutral setup inputs are scoped in
[Package setup](package-setup.md). The first slice now signs extensible input
descriptors with releases, exposes them through `GetPackage`, and accepts and
validates per-install values through npackd `Install`. Platform-specific type
interpretation and applying those values remain client/integration work.

Do this before connecting a graphical store. Separate user and system
operations:

```text
npackd-user    → ~/.local packages
npackd-system  → /usr /etc /var
```

Use PolicyKit for privileged installation rather than running a frontend or
Nostr stack as root. Formalise publisher trust, revocation, capability
declarations, file ownership/conflicts, rollback, transaction locking, and
interrupted-install recovery.

Goal: a credible security model for distro-facing integration.

Shipped: transaction locking, rollback, and interrupted-install recovery.
Every mutating store operation (`install`, `remove`, and the daemon's
`Install`/`Remove`/`Update`) is wrapped in a `StoreTransaction`: it takes an
exclusive `npack.lock` on the store root (so two operations against the same
store can't race on `installed.json` or a package directory -- a concurrent
attempt fails fast with "another npack operation is already in progress"
rather than corrupting state), snapshots `installed.json` before mutating
it, and records a small journal noting which package directory the
transaction is about to create. If the operation returns an error, the
transaction's `Drop` restores `installed.json` from the snapshot and removes
the package directory if the transaction had created it -- the same
mechanism Rust already used for a single package's file-level rollback
(`install_staged_npk`'s backup-and-restore), now applied at the whole-store
level. If the process is killed outright (`kill -9`, a crash, a power loss)
mid-transaction, the lock, journal, and backup are left on disk; the next
`install` or `remove` against that store notices the lock's pid is no longer
alive, treats this as an interrupted transaction, and performs the same
restore-and-clean-up before proceeding -- so a killed npack process cannot
leave a store's bookkeeping pointing at a half-written package. This does
not undo a `remove`'s file deletions themselves (only `installed.json`
consistency), since that would require backing up every file before
deleting it; formalising file ownership/conflicts already existed
(`ensure_install_paths_available`, `remove_stale_files`) and was not part of
this slice.

Shipped: the `npackd-user`/`npackd-system` daemon split and PolicyKit-gated
privileged installation. `npack daemon` (unchanged) is npackd-user, serving a
single user's own `~/.local` store with no additional gating -- the daemon
only ever acts on behalf of the account that started it. `npack daemon
--system` is npackd-system: it refuses to start unless run as root (normally
as the systemd system service in `packaging/npackd-system.service`), binds a
world-connectable socket (`/run/npackd.sock`, mode 0666, matching how the
system D-Bus and PackageKit sockets work), and reads each connecting peer's
real uid and pid off the Unix socket itself via `SO_PEERCRED`
(`UnixStream::peer_cred`) rather than trusting anything the client claims.

A root peer of npackd-system is already fully privileged and is never
gated. A non-root peer's `Install`, `Remove`, or `Update` request is checked
against PolicyKit before it runs: npackd-system shells out to `pkcheck
--action-id io.npack.<install|remove|update> --process <peer-pid>
--allow-user-interaction`, which talks to `polkitd` over D-Bus on npack's
behalf and can trigger a graphical authentication prompt via whatever
polkit agent the peer's session is running. The three action ids are
defined in `packaging/io.npack.policy` (installed to
`/usr/share/polkit-1/actions/` by system packaging -- not written by npack
at runtime) and default to `auth_admin`, the same "requires an administrator
password" default PackageKit itself uses for `package-install`. A peer whose
credentials can't be read at all is denied rather than treated as
authorized, and `GetTransaction`/`CancelTransaction`/read-only methods are
never gated, since cancelling only flips a flag the already-authorized
transaction's own loop checks.

Chose shelling out to `pkcheck` over a `zbus`/`zbus_polkit` D-Bus client: it
is a single request/response authorization check, `pkcheck` is the tool
PolicyKit itself ships for exactly this, and it avoids adding a D-Bus client
library and its own async runtime integration for one call. Verified against
the real `pkcheck`/`polkitd` on a machine with PolicyKit actually installed
and running, not a mock authority: an unregistered action correctly denies a
non-root peer before any package resolution happens (this ships as an
automated test using the genuine `pkcheck` binary in CI), and a locally
added `allow_active: yes` polkit rule for `io.npack.install` was confirmed
to let the same request through -- so both the deny and the allow paths were
exercised against real infrastructure, not simulated.

Remaining work for this phase: richer publisher trust/revocation and
capability-declaration formalisation.

## Phase 4: App catalogue and index (done)

Build a local catalogue that can be rebuilt from Nostr:

```text
Nostr relays → npack catalogue
                  ├─ package metadata
                  ├─ AppStream metadata
                  ├─ publisher
                  ├─ latest versions
                  ├─ platform compatibility
                  └─ trust/revocation state
```

Add:

```bash
npack refresh
npack search firefox
npack info <publisher>/firefox
```

Interactive browsing should not query relays directly for every operation.

Shipped: `npack refresh` fetches every release and revocation event from the
configured/given relays and writes them into a durable local catalogue file
(`<store>/catalogue.json`), replacing the old per-query, 5-minute-TTL search
cache entirely. `npack search` now reads only this catalogue by default --
zero relay round trips for a normal browsing session -- with `--refresh`
rebuilding the catalogue from relays first and `--no-cache` doing a one-off
live relay query without touching the catalogue file at all, so the old
escape hatches for "I want fresh results right now" still exist, just
repointed at the catalogue model. `npack info [<publisher>/]<name>` is new:
it lists every known version across every publisher and os/arch
combination for a package, newest first, annotated with revocation state
and (given `--trusted-publisher`) trust status -- reading the catalogue only,
with no relay fallback, since a single package's full picture is exactly
what the catalogue exists for.

Verified against real infrastructure, not simulated: `npack refresh`
against `wss://relay.damus.io` and `wss://nos.lol` populated a real
catalogue (500 releases fetched), `npack search npack` then found npack's
own published releases from that catalogue in single-digit milliseconds
with no relay traffic, `npack info npack` listed three real published
versions with correct revocation/trust annotations, `--no-cache` was
confirmed to leave the catalogue file's mtime untouched while still
returning live results, `--refresh` was confirmed to update it, and the
daemon's `Search` method was confirmed to serve the same catalogue over a
real Unix socket.

One gap knowingly left open: the roadmap's "AppStream metadata" column in
the catalogue diagram above isn't populated. Phase 1's `app` manifest
metadata (summary, description, icon, ...) is used locally by `npack
appstream`, but publishing it into Nostr release-event tags was never
built, so there is currently nothing for a relay-sourced catalogue to
carry. `npack info` reflects this honestly by not fabricating AppStream
fields; publishing `app` metadata to Nostr is unstarted future work, likely
alongside Phase 5's reference GUI once there is a consumer that wants it.

## Phase 5: Reference GUI (done)

Build a small npack GUI to exercise the service API:

- Search
- Details
- Install
- Installed
- Updates
- Remove

This proves that `npackd` is frontend-independent before integrating with an
existing store.

Shipped: `npack-gui`, a small `egui`/`eframe` desktop app in its own crate
(the repo is now a Cargo workspace: `npack-cli` for the existing CLI/daemon,
`npack-gui` for this). It talks to npackd purely over the documented
newline-delimited-JSON Unix-socket protocol (`npack-gui/src/client.rs`) --
it has no dependency on and no special access to the `npack` crate's
internals, so it is exactly as frontend-independent a client as a COSMIC,
GNOME, or KDE integration would be.

Three tabs cover the roadmap list: **Installed** (`ListInstalled`, with
per-package Details/Remove), **Search** (`Search`, with a checkbox to
rebuild the local catalogue from relays first via the existing
`"refresh": true` param, and per-result Details/Install), and **Updates**
(`CheckUpdates`). **Details** calls `GetPackage` and renders the raw
response. **Install** uses `"async": true` and polls `GetTransaction` on a
background thread, so the status bar shows real progress
(`connecting`/`resolving`/`downloading`/`installed`) exactly as Phase 2
designed it to. Every daemon call runs on a background thread and reports
back over a channel, so a slow relay or Blossom fetch never freezes the
window.

Verified: the workspace restructure was validated by confirming
`cargo build --release`, `cargo run -- ...` (via `default-run`), `cargo
test`, and `cargo fmt`/`clippy` at the workspace root all still work
exactly as the CI and release workflows invoke them, unchanged. The GUI
itself was verified against a real running `npack daemon`: the binary
launches and stays alive with no panics, and a hand-sent request over the
real daemon's socket (`nc -U`) confirmed the exact response shape
`DaemonClient` expects. Screenshot capture is blocked in the environment
this was built in, so the actual rendered window was not visually
inspected as part of this work -- a reasonable next step for whoever picks
up Phase 6 or 7 is a manual click-through on a normal desktop.

## Phase 6: COSMIC Store integration

Investigate COSMIC first because its newer Rust-oriented codebase should make
experimentation and a potential upstream provider easier. Keep the integration
as a backend/provider adapter:

```text
COSMIC search → npackd Search()
Install button → npackd Install() → PolicyKit → verified .npk
```

Avoid a permanent fork unless unavoidable.

## Phase 7: GNOME Software

Once the backend is stable, provide a thin GNOME Software plugin:

```text
GNOME Software plugin → D-Bus → npackd
```

Translate publisher, release, revocation, trust, and transport details into
desktop-store concepts while keeping Nostr and Blossom logic in `npackd`.

## Phase 8: KDE Discover

Reuse the same adapter model for KDE Discover. At this point the work should
primarily be UI translation:

```text
                 npackd
           ┌──────┼──────┐
           ▼      ▼      ▼
        COSMIC  GNOME  Discover
```

## Phase 9: Update integration

Add the desktop update experience:

- Periodic update checks and notifications.
- Offline update support where appropriate.
- Staged downloads.
- Security-update classification.
- Update history and rollback hooks.

Release metadata may express `normal`, `recommended`, `security`, or
`critical`, but publishers should not be the sole authority for security
classification. Curator and distro policy should remain possible inputs.

## Phase 10: Trust and curation

Support trusted package sets such as recommended publishers, Fedora community
sets, Nostr developer tools, or a user's personal trusted list. Let the GUI
distinguish:

```text
Verified upstream / Community maintained / Unknown publisher / Revoked
```

Do this without introducing a mandatory central repository.

## Phase 11: Build provenance in the GUI

Surface existing provenance data in application details:

```text
Publisher          Verified Nostr identity
Source             Repository and source commit
Build              Builder and workflow
Provenance         Verified
Artifact hash      Verified
Reproducible       Matching independent builders
```

## Phase 12: Distro integration

Approach distributions only after the backend works across multiple desktop
stores. The pitch is an additional decentralised, cryptographically verified
software source using the existing desktop experience—not a replacement for
APT, RPM, or Flatpak.

## Declarative and immutable OS integration

Runs independently of the desktop-store phases above, for consumers that want
to manage the resulting installation themselves (Nix, Guix, immutable/atomic
distros) rather than have npack write to the filesystem.

Shipped (originally requested for NixOS integration, see
[issue #1](https://github.com/imattau/npack/issues/1)):

- `npack resolve [<publisher>/]<name> --relay <relay-url>` (v0.2.10) performs
  the same relay discovery, trust/semver/os-arch filtering, revocation check,
  and NIP-94 artifact-event verification as `install-ref`, but stops before
  downloading the artifact or installing anything.
- Prints the resolved publisher key, name, version, SHA-256, candidate
  artifact URLs, declared dependencies, and a verification summary as JSON,
  so a derivation generator can fetch and unpack the artifact itself while
  npack continues to own Nostr/Blossom discovery and trust.
- `npack resolve --recursive` walks the full declared dependency closure in
  one call (cycle and version-conflict checks included) and prints a JSON
  array of resolved entries, so a derivation generator does not need a
  separate invocation per dependency.

Validated: [a real Nix derivation built from `npack resolve`'s output against
npack's own live-published release](nixos-integration-test.md), including a
negative control confirming Nix rejects a tampered SHA-256.

Remaining work:

- Validation against a full NixOS system closure (a flake-based module or
  package entry, not just a one-off derivation), and a package that actually
  declares dependencies to exercise `npack resolve --recursive`, ideally with
  help from the issue's reporter.

## Milestone order

The priority sequence is:

```text
Phases 1–5  →  prove npackd and the reference GUI
Phases 6–8  →  desktop-store adapters
Phases 9–11 →  mature updates, trust, and provenance UX
Phase 12    →  distro-facing integration
```
