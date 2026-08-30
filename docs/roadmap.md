# npack development roadmap

The near-term objective is to prove npack as a frontend-independent package
service before committing to any particular desktop store. The first major
milestone is Phases 1–5: metadata compatibility, a stable service layer,
security and privilege separation, a local catalogue, and a small reference
GUI.

## Phase 1: Package metadata compatibility

Add first-class AppStream support:

- Map `.npk` metadata to AppStream fields.
- Support icons, screenshots, categories, homepage, licence, summary, and description.
- Add desktop-file validation.
- Add `npack appstream <package>` output.
- Validate against AppStream tooling in CI.

Goal: an npack package can describe itself in the language already understood
by Linux application stores.

## Phase 2: Stable service layer

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

Core operations:

```text
Search()          GetPackage()
ListInstalled()   Install()
Remove()          Update()
CheckUpdates()    GetTransaction()
CancelTransaction()
```

Expose progress and transaction events. GUI integrations should not need to
understand Nostr, Blossom, or `.npk` internals.

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

## Milestone order

The priority sequence is:

```text
Phases 1–5  →  prove npackd and the reference GUI
Phases 6–8  →  desktop-store adapters
Phases 9–11 →  mature updates, trust, and provenance UX
Phase 12    →  distro-facing integration
```
