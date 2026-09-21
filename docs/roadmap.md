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

## Phase 2: Stable service layer (in progress)

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

Remaining work:

- Streamed progress events (e.g. per-file download progress) rather than a
  single final result once `GetTransaction` reports the transaction done.

## Phase 3: Security and privilege separation

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

## Phase 4: App catalogue and index

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

## Phase 5: Reference GUI

Build a small npack GUI to exercise the service API:

- Search
- Details
- Install
- Installed
- Updates
- Remove

This proves that `npackd` is frontend-independent before integrating with an
existing store.

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
