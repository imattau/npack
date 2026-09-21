# Using npack

`npack` is a Nostr-native package manager. It discovers signed release
metadata on Nostr relays, downloads immutable `.npk` files from Blossom or
other HTTP-compatible storage, verifies the artifact hash, resolves
dependencies, and installs files using the host operating system's normal
filesystem layout.

The project is an early prototype. The package event format is a project
protocol and is not yet a registered NIP.

## Install npack

Build from source:

```bash
git clone https://github.com/imattau/npack.git
cd npack
cargo build --release
install -m 0755 target/release/npack ~/.local/bin/npack
```

Check the installation:

```bash
npack --version
```

The first package is `npack` itself. A bootstrap binary can install later
versions of `npack` in the same way as any other package.

## Installation modes

System installation is the default:

```bash
npack install ./myapp-1.0.0.npk
```

This installs into the host filesystem, normally requiring administrator
privileges. Package state is stored in `/var/lib/npack`.

For an unprivileged user-local installation:

```bash
npack install ./myapp-1.0.0.npk --user
```

This uses the host's conventional `~/.local` prefix and user-local npack
state. It does not create a separate application directory or replace the
host's filesystem conventions.

For development and tests, use an isolated store:

```bash
npack install ./myapp-1.0.0.npk --store /tmp/npack-store
```

`--user` and `--system` are mutually exclusive. `--system` is the explicit
form of the default.

## Installing a local `.npk`

An `.npk` is a deterministic tar archive compressed with zstd. A package
contains its local installation metadata at `.npack/manifest.json`:

```text
.npack/manifest.json
bin/myapp
lib/libmyapp.so
share/myapp/README.md
```

Install it directly:

```bash
npack install ./myapp-1.0.0.npk --user
```

`npack` reads the embedded metadata, calculates the archive SHA-256, verifies
the package contents, checks dependencies and conflicts, and records the
installed files. The `.npack` metadata directory is not installed into the
target prefix.

Standalone local installation proves archive integrity, but it does not
prove who published the package. For publisher authentication, install from
Nostr using a signed release event.

## Creating an `.npk`

Start a package scaffold with:

```bash
npack init ./myapp \
  --name myapp \
  --version 1.0.0 \
  --publisher npub1...
```

This creates `.npack/manifest.json` with the host OS and architecture. Use
`--os` and `--arch` to target another platform, for example
`--os linux --arch aarch64`. Add payload files beneath the scaffold, then
pack it:

```bash
npack pack ./myapp --output ./myapp-1.0.0-linux-x86_64.npk
```

Create a package root using normal host paths:

```text
package-root/
├── .npack/
│   └── manifest.json
├── bin/
│   └── myapp
└── share/
    └── myapp/
        └── README.md
```

The embedded manifest describes the package. Its `sha256` is left empty when
building because the archive hash is not known until the archive is complete:

```json
{
  "publisher": "npub1...",
  "name": "myapp",
  "version": "1.0.0",
  "artifact": "myapp-1.0.0.npk",
  "sha256": "",
  "dependencies": [],
  "conflicts": [],
  "os": "linux",
  "arch": "x86_64",
  "format": "npk",
  "runtime_requires": [],
  "provides": [],
  "post_install": []
}
```

Build and inspect the archive:

```bash
npack pack ./package-root --output ./myapp-1.0.0.npk
npack manifest ./myapp-1.0.0.npk \
  --output ./myapp-1.0.0.manifest.json
npack hash ./myapp-1.0.0.npk
npack inspect ./myapp-1.0.0.npk
npack verify ./myapp-1.0.0.npk
```

`npack pack` preserves directory structure, executable permissions and
symlinks. It normalizes timestamps and ownership and sorts entries so the
same package root produces the same archive.

The package root should contain payload paths relative to the host root:
`bin/`, `lib/`, `share/`, `etc/`, and so on. Do not put `/usr` or `/home`
inside the archive unless that is deliberately part of the target layout.

## Manifest fields

The important fields are:

| Field | Purpose |
| --- | --- |
| `publisher` | Nostr public key identifying the publisher; use an `npub` for user-facing configuration. |
| `name` | Package name within the publisher namespace. |
| `version` | Semantic version. |
| `artifact` | Archive filename. |
| `sha256` | Final archive hash in a published manifest; empty only in embedded build metadata. |
| `dependencies` | Other npack packages and version requirements. |
| `conflicts` | Packages that cannot coexist with this package. |
| `os`, `arch` | Compatibility target, such as `linux` and `x86_64`, or `any`. |
| `runtime_requires` | Host capabilities needed by the package, such as ELF libraries. |
| `provides` | Capabilities supplied to other packages. |
| `post_install` | Declarative, capability-gated installation actions. |
| `app` | Optional desktop-store metadata: `summary`, `description`, `homepage`, `license`, `categories`, `icon`, `screenshots`, `desktop_file`, `release_date`. See [AppStream metadata](#appstream-metadata) below. |

An external publishing manifest must contain the final SHA-256:

```bash
sha256=$(npack hash ./myapp-1.0.0.npk)
```

The external manifest is used by the publishing and verification commands;
end users normally receive equivalent release metadata from Nostr.

Generate an external release manifest from a completed archive instead of copying the embedded
metadata by hand:

```bash
npack manifest ./myapp-1.0.0.npk \
  --output ./myapp-1.0.0.manifest.json
```

## AppStream metadata

A manifest's optional `app` object describes the package in the language
Linux application stores already understand:

```json
{
  "publisher": "npub1...",
  "name": "myapp",
  "version": "1.0.0",
  "artifact": "myapp-1.0.0.npk",
  "sha256": "",
  "app": {
    "summary": "A friendly greeting",
    "description": "Prints a friendly greeting to the terminal.",
    "homepage": "https://example.com/myapp",
    "license": "MIT",
    "categories": ["Utility"],
    "icon": "share/icons/myapp.png",
    "screenshots": ["https://example.com/myapp/screenshot.png"],
    "desktop_file": "myapp.desktop",
    "release_date": "2026-01-15"
  }
}
```

`icon` and `desktop_file` are package-relative paths to files included in the
`.npk`; `npack pack` checks both exist in the source directory, and validates
`desktop_file` as a syntactically correct freedesktop.org
[Desktop Entry](https://specifications.freedesktop.org/desktop-entry-spec/latest/)
file (a `[Desktop Entry]` group with `Type` and `Name`, plus `Exec` when
`Type=Application`).

Generate an [AppStream](https://www.freedesktop.org/software/appstream/docs/)
component document from a built archive:

```bash
npack appstream ./myapp-1.0.0.npk --output ./myapp.metainfo.xml
```

Packages without a `desktop_file` are rendered as a `console-application`
component advertising `<provides><binary>myapp</binary></provides>`; packages
with one are rendered as a `desktop-application` component advertising
`<launchable type="desktop-id">myapp.desktop</launchable>`. Component IDs are
namespaced `io.npack.<publisher>.<name>`, since npack publishers are Nostr
public keys rather than domains. Validate the generated document with
[`appstreamcli`](https://www.freedesktop.org/software/appstream/docs/man/appstreamcli.1.html):

```bash
appstreamcli validate --no-net ./myapp.metainfo.xml
```

## Dependencies and install order

Dependencies use package names and semantic version requirements:

```json
"dependencies": [
  { "name": "libfoo", "requirement": ">=2.0.0" },
  { "publisher": "npub1...", "name": "helper", "requirement": "^1.4" }
]
```

The publisher-qualified form is preferred when a name could be ambiguous.
The resolver:

1. Finds a compatible signed release.
2. Resolves its dependencies recursively.
3. Detects cycles and conflicts.
4. Installs dependencies before the requested package.
5. Prints and records the resulting order.

Write a lockfile for repeatable deployments:

```bash
npack install npub1.../myapp \
  --relay wss://relay.example \
  --lockfile npack.lock
```

Replay the exact locked graph offline after artifacts and metadata have been
cached:

```bash
npack update npub1.../myapp \
  --lockfile npack.lock \
  --locked \
  --offline
```

## Discovering and installing from Nostr

Build the local catalogue once, then browse it with no further relay
round trips:

```bash
npack refresh --relay wss://relay.example
npack search myapp
npack info myapp
```

`npack search` reads the catalogue file (default `<npack state dir>/
catalogue.json`, or `--store <path>` for an isolated one) by default and
never touches a relay unless asked:

```bash
npack search myapp --relay wss://relay.example --refresh   # rebuild the catalogue from relays, then search it
npack search myapp --relay wss://relay.example --no-cache  # query relays live for this search only, without touching the catalogue
```

If no catalogue exists yet, plain `npack search` fails with a message
pointing at `npack refresh` rather than silently querying relays.

Install a publisher-qualified package:

```bash
npack install npub1.../myapp \
  --relay wss://relay.example \
  --user
```

`install-ref` remains accepted as a compatibility alias for scripts using the
older command name. The remote installation process is:

1. Query relays for signed `kind:9900` release events.
2. Verify the Nostr signature and release fields.
3. Reject revoked or incompatible releases.
4. Resolve dependencies and determine install order.
5. Fetch the signed NIP-94 `kind:1063` artifact metadata.
6. Try the publisher's Blossom server list and configured fallback servers.
7. Verify the downloaded bytes against the release SHA-256.
8. Install the verified `.npk`.

Relays and storage servers are transport. They do not become package
authorities merely because they served an event or file.

Each step prints progress to stderr (connecting, resolving, downloading) so
the process doesn't look hung during slow relay or Blossom lookups, and each
installed package prints a confirmation to stdout on success:

```text
Connecting to 1 relay(s)... done in 0.4s
Resolving npub1.../myapp...
Downloading npub1.../myapp 1.0.0 (2 mirror(s))... done in 1.1s (48213 bytes)
installed npub1.../myapp 1.0.0
install order: npub1.../myapp
```

To check every package in the selected install store for a newer release:

```bash
npack update --user
npack update --system
```

The global update command uses the installed publisher/name and requires a
strictly newer version, then reuses normal dependency resolution, artifact
hash verification, and install ordering. A targeted update remains available
with `npack update <publisher>/package`.

Pass `--check` to report available updates without installing anything, the
`apt update` equivalent to `update`'s `apt upgrade`:

```bash
npack update --user --check
npack update <publisher>/myapp --relay wss://relay.example --check
```

For each installed package this resolves and verifies the newest release
matching a strictly newer version, the same way `update` does, but stops
before downloading the artifact or installing it. Output looks like:

```text
npub1.../myapp 1.0.0 -> 1.1.0 available
npub1.../otherapp 2.3.0 up to date
1 update(s) available.
```

Search results are reduced to the newest valid SemVer release for each
publisher/package pair, while retaining all platform artifacts belonging to
that release; a `--trusted-publisher` filter and NIP-65 identity narrow which
catalogue entries are considered, same as before.

### `npack info`: a package's full picture from the catalogue

```bash
npack info myapp
npack info npub1.../myapp
```

Unlike `search`, `info` has no `--refresh`/`--no-cache` escape hatch to
relays -- it always reads the local catalogue, since showing every known
version/publisher/platform combination for one package is exactly what the
catalogue is for. Output lists each publisher (optionally annotated
`(trusted)`/`(not in trusted-publisher list)` when `--trusted-publisher` is
given), then every version for that publisher newest first, its os/arch, its
release event id, and `[REVOKED]` when applicable:

```text
myapp
  publisher npub1... (trusted)
    1.2.0 linux/x86_64 76709197...
    1.1.0 linux/x86_64 895abc98... [REVOKED]
```

AppStream-style fields (summary, description, homepage, ...) are not shown:
release events don't currently carry Phase 1's `app` manifest metadata onto
Nostr tags, so there is nothing for the catalogue to have captured yet.

## Resolving release metadata for other package managers

`npack resolve` performs the same relay discovery, trust and revocation
checks, and signature verification as remote installation (steps 1-3 and 5
above), but stops before downloading the artifact or installing anything:

```bash
npack resolve npub1.../myapp --relay wss://relay.example
```

The command prints the resolved publisher, name, version, SHA-256, candidate
artifact URLs (NIP-94 `url` tags plus the publisher's declared Blossom
servers), declared dependencies, and a verification summary as JSON on
stdout, for example:

```json
{
  "publisher": "...",
  "name": "myapp",
  "version": "1.0.0",
  "sha256": "...",
  "os": "linux",
  "arch": "x86_64",
  "format": "npk",
  "artifact_urls": ["https://blossom.example/..."],
  "dependencies": [],
  "conflicts": [],
  "runtime_requires": [],
  "provides": [],
  "release_event_id": "...",
  "artifact_event_id": "...",
  "verification": {
    "release_signature_valid": true,
    "artifact_event_signature_valid": true,
    "release_event_is_v1": true,
    "publisher_trusted": true,
    "revoked": false
  }
}
```

This is intended for declarative package managers (such as Nix) that want to
own fetching, unpacking, and rollback of the artifact themselves while
leaving Nostr/Blossom discovery and trust verification to npack. `--relay`,
`--requirement`, `--trusted-publisher`, `--pubkey`, `--lockfile`, and
`--locked` behave the same as their `install-ref` counterparts. See
[the Nix integration test](nixos-integration-test.md) for a worked example
building a real Nix derivation from this output.

Pass `--recursive` to resolve the full dependency closure in one call instead
of a single package. The command then walks each declared dependency the same
way `install-ref` would, but only resolves and verifies metadata rather than
installing, and prints a JSON array of resolved entries -- one per
publisher/package in the graph -- instead of a single object:

```bash
npack resolve npub1.../myapp --relay wss://relay.example --recursive
```

Dependency cycles are rejected the same way they are during installation, and
a package required at incompatible versions by two different entries in the
graph fails resolution rather than silently picking one. This is the shape a
derivation generator would want to build one `fetchurl`-plus-unpack
derivation per package in the graph, without needing a separate `npack
resolve` invocation for every dependency.

## Running npackd (local service API)

`npack daemon` runs npackd, a common local backend that a GUI store or other
tool can talk to without needing to understand Nostr, Blossom, or `.npk`
internals -- the CLI itself is one possible client:

```bash
npack daemon --socket $XDG_RUNTIME_DIR/npackd.sock
```

`--socket` defaults to `$XDG_RUNTIME_DIR/npackd.sock`. The protocol is
newline-delimited JSON over that Unix socket: one JSON object request per
line, one JSON object response per line, matched by `id`.

```text
--> {"id": 1, "method": "ListInstalled", "params": {"user": true}}
<-- {"id": 1, "result": [{"publisher": "npub1...", "name": "myapp", "version": "1.0.0", ...}]}

--> {"id": 2, "method": "GetPackage", "params": {"package": "npub1.../myapp", "relay": ["wss://relay.example"]}}
<-- {"id": 2, "result": {"publisher": "...", "name": "myapp", "version": "1.2.0", "sha256": "...", ...}}
```

Supported methods, mirroring the CLI operations above:

| Method | Params | Result |
| --- | --- | --- |
| `Search` | `query`, `relay[]`, `trusted_publisher[]`, `pubkey`, `refresh`, `no_cache` | Array of matching releases. |
| `GetPackage` | `package`, `relay[]`, `requirement`, `os`, `arch`, `trusted_publisher[]`, `store`, `user` | The same resolved-metadata object as `npack resolve`. |
| `ListInstalled` | `user`, `store` | Array of installed packages. |
| `Install` | `package`, `requirement`, `relay[]`, `server[]`, `user`, `store`, `allow_capability[]`, `async` | The installed package's record, or `{"transaction_id": N}` if `async` is true. |
| `Remove` | `package`, `user`, `store` | `{"removed": "<package>"}`. |
| `Update` | `package` (omit for all), `relay[]`, `server[]`, `user`, `store`, `allow_capability[]`, `async` | Array of per-package update outcomes, or `{"transaction_id": N}` if `async` is true. |
| `CheckUpdates` | `package` (omit for all), `relay[]`, `trusted_publisher[]`, `user`, `store` | Array of `{reference, current_version, available_version}`. |
| `GetTransaction` | `transaction_id` | `{"status": "running", "progress": {...}}`, `{"status": "succeeded", "result": ...}`, `{"status": "failed", "error": "..."}`, or `{"status": "cancelled"}`. |
| `CancelTransaction` | `transaction_id` | `{"cancel_requested": true}`. |

An unknown method or a request that fails to deserialize its params returns
`{"id": ..., "error": "..."}` instead of `result`. Each connection is handled
concurrently.

By default `Install` and `Update` run to completion before responding. Pass
`"async": true` to get `{"transaction_id": N}` back immediately and poll
`GetTransaction` for progress and the final result:

```text
--> {"id": 1, "method": "Install", "params": {"package": "npub1.../myapp", "relay": ["wss://relay.example"], "async": true}}
<-- {"id": 1, "result": {"transaction_id": 1}}

--> {"id": 2, "method": "GetTransaction", "params": {"transaction_id": 1}}
<-- {"id": 2, "result": {"status": "running", "progress": {"stage": "resolving", "package": "npub1.../myapp"}}}
   ... later ...
--> {"id": 3, "method": "GetTransaction", "params": {"transaction_id": 1}}
<-- {"id": 3, "result": {"status": "running", "progress": {"stage": "downloading", "package": "npub1.../myapp", "detail": "3 mirror(s)"}}}
   ... later ...
--> {"id": 4, "method": "GetTransaction", "params": {"transaction_id": 1}}
<-- {"id": 4, "result": {"status": "succeeded", "result": {"publisher": "...", "name": "myapp", "version": "1.0.0", ...}}}
```

`progress.stage` while `running` is one of `connecting`, `resolving`,
`downloading`, `updating` (during `Update`), or `installed`, each paired with
the publisher/name of the package currently being processed. This is
per-package status, not a per-byte download progress bar -- it is updated at
the same checkpoints `CancelTransaction` is checked at (see below), not on
every downloaded chunk.

`CancelTransaction` is cooperative, not forcible: it is only checked between
packages (before starting the next package in a dependency graph, or the
next package in an `Update` loop), never mid-download or mid-install of a
package already in progress. This means a package that has already started
installing will finish before cancellation takes effect -- the store can
never be left half-installed by a cancelled transaction.

### npackd-user vs. npackd-system, and PolicyKit

Plain `npack daemon` is npackd-user: it runs as your own account and serves
only your own `--user`/default store (`~/.local`) -- there is nothing to gate
since it never acts on anyone else's behalf.

```bash
npack daemon --system
```

`npack daemon --system` is npackd-system: a privileged instance intended to
run as a systemd system service (`packaging/npackd-system.service`), always
as root, serving the system store (`/`, state in `/var/lib/npack`). It
refuses to start unless it is actually running as root. Because its socket
(`/run/npackd.sock` by default) is left world-connectable, it authorizes
each request itself rather than relying on socket permissions: a root peer
is trusted outright, but a non-root peer's `Install`, `Remove`, or `Update`
is checked against PolicyKit before it runs, using the peer's real
credentials read off the socket (`SO_PEERCRED`), not anything the client
claims. This is the same model PackageKit and other system package
services use.

The three actions -- `io.npack.install`, `io.npack.remove`, `io.npack.update`
-- are defined in [`packaging/io.npack.policy`](../packaging/io.npack.policy),
which a system package installs to `/usr/share/polkit-1/actions/`. They
default to `auth_admin`, so a non-root caller normally sees their desktop's
usual "authenticate as an administrator" prompt (handled by whatever
PolicyKit agent their session runs) the first time they ask npackd-system to
install, remove, or update something. `GetTransaction`, `CancelTransaction`,
and the read-only methods are never gated.

## Configuration

Configuration is stored at the platform's user config path, normally:

```text
$XDG_CONFIG_HOME/npack/config.toml
```

Example:

```toml
[network]
relays = [
  "wss://relay.example",
  "wss://relay2.example",
]

[storage]
blossom = [
  "https://blossom.example",
]

[identity]
pubkey = "npub1..."

[trust]
publishers = ["npub1...", "npub1..."]

[install]
user = false
```

When `identity.pubkey` is configured, npack reads that user's NIP-65
`kind:10002` relay list and adds read-capable relays. The identity is also
used to discover publisher Blossom servers through `kind:10063` events.

Publisher keys may be written as `npub` values. Internally, npack compares
canonical hexadecimal public keys so equivalent representations cannot create
separate identities.

## Trust and security

Trust publishers explicitly when possible:

```bash
npack search myapp \
  --trusted-publisher npub1... \
  --relay wss://relay.example
```

The v1 security model uses:

- A publisher-signed Nostr release event (`kind:9900`).
- A publisher-signed NIP-94 artifact event (`kind:1063`).
- SHA-256 verification of every downloaded archive.
- Signed revocation events (`kind:9901`).
- Explicit dependency, conflict and runtime capability declarations.

Do not use a personal primary Nostr key in automated publishing. Use a
dedicated package-publisher key and protect it carefully. Delegated release
keys are not part of protocol v1.

Post-install actions are deliberately limited and capability-gated. A
service file may be installed, but services are not automatically enabled or
started. Grant capabilities explicitly with:

```bash
npack install ./myapp.npk \
  --allow-capability service-manager
```

Review package metadata and publisher identity before granting capabilities.

## Publishing a release

### Registering a publisher key

Generate a new publisher key and store its `nsec` automatically in the
operating system credential store:

```bash
npack generate-key
```

The public `npub` is displayed; the private key is not printed by default.
Use `--show-secret` only when you are prepared to copy and back up the `nsec`
securely:

```bash
npack generate-key --show-secret
```

Register a dedicated publisher key once in the operating system credential
store. With no argument, npack prompts without echoing the key:

```bash
npack register
```

The key is stored through the platform credential APIs: Secret Service on
Linux, Keychain on macOS, or Credential Manager on Windows. It is not written
to `config.toml`. The `--stdin` form avoids exposing the nsec in shell history
or the process list. A positional form is also available, but is less safe:

```bash
npack register nsec1...
```

After registration, `release-event`, `publish`, and `revoke-event` can omit
`--secret-key`. Supplying `--secret-key` explicitly always overrides the
registered key. Registering another key replaces the previous `npack`
publisher credential for the current user.

Prepare an external manifest whose `artifact` and `sha256` identify the final
`.npk`, then create and verify the release metadata:

```bash
npack release-event ./myapp.manifest.json \
  --secret-key <publisher-secret-key> \
  > release.json

npack verify ./myapp.manifest.json
npack verify-event release.json ./myapp.manifest.json
```

Publish the artifact and both Nostr events:

```bash
npack publish ./myapp.manifest.json \
  --relay wss://relay.example \
  --server https://blossom.example
```

`npack publish` uploads the `.npk`, creates the NIP-94 artifact event, creates
the package release event, and sends both events to the configured relays.

To revoke a published release:

```bash
npack revoke-event release.json \
  --secret-key <publisher-secret-key> \
  --reason "security issue"
```

Deletion is not treated as revocation because relays and clients may retain
copies of the original event.

## GitHub Actions

The repository contains a reusable reference workflow at
`.github/workflows/release.yml`. On a `v*` tag it builds a deterministic
Linux `.npk`, embeds package metadata, calculates the final hash, generates
an SPDX SBOM, creates a GitHub provenance attestation, uploads a GitHub
Release, and publishes to Nostr.

Configure:

- Repository variable `NOSTR_PUBLISHER` — the publisher public key.
- Repository variables `NOSTR_RELAYS` and `NOSTR_BLOSSOM_SERVERS` — one URL
  per line.
- Secret `NOSTR_SECRET_KEY` in the protected `release` environment — the
  dedicated publisher private key.

GitHub Actions is only a builder and publisher implementation. Forgejo,
GitLab, local builders and reproducible-build systems can publish the same
`.npk` and Nostr event format.

## Package maintenance

List installed packages:

```bash
npack list --user
```

Verify installed files and hashes:

```bash
npack verify-installed --user
```

Update a remote package:

```bash
npack update npub1.../myapp --user
```

Remove a package:

```bash
npack remove npub1.../myapp --user
```

Removal is refused when another installed package depends on the package or
when removing it would leave a required runtime capability unavailable.

### Transaction locking and interrupted-install recovery

Every `install`/`remove` against a given store (system, `--user`, or a
`--store <path>` used for isolated/test installs) is serialized by an
exclusive lock, so running two npack operations against the same store at
once fails fast rather than racing:

```text
Error: another npack operation is already in progress (pid 12345)
```

If an npack process is killed mid-install or mid-remove (a crash, `kill -9`,
a power loss), the next operation against that store detects that the pid
holding the lock is no longer running, prints a recovery notice, restores
`installed.json` to its state before the interrupted transaction, and
removes any package directory that transaction had newly created -- then
proceeds normally. No manual cleanup of the store is needed after an
interrupted operation.

## Troubleshooting

### `no verified release found`

Add more package relays with `--relay`, use the publisher-qualified package
reference, or check that the publisher is not filtered out by a trust list.

### `no artifact mirror returned the expected SHA-256`

The storage server may be unavailable or may not contain the blob. Add a
known Blossom server with `--server`. A server returning different bytes is
rejected automatically.

### Permission denied during installation

Use `--user` for a user-local install, or run the default system install with
the privileges required by the host filesystem.

### Dependency or runtime capability failure

Read the package's declared dependencies and `runtime_requires`. Install the
missing package/provider first, or use a release built for the correct host
OS and architecture.

## Command summary

Publish a signed Nostr text note using the registered key and configured
write relays:

```bash
npack announce "npack is now available!"
```

Use `--secret-key` for a one-off key override, or `--relay` to select relays
explicitly. To attach a signed package release event, pass its JSON file with
`--release-event`. The command adds package name, version, platform, SHA-256,
a `nostr:nevent...` link, and NIP-27-compatible event tags automatically:

```bash
npack announce --release-event ./release.json "npack 0.2.6 is available!"
```

Generate the installed command reference:

```bash
npack man > npack.1
man ./npack.1
```

Release packages install the same page as `share/man/man1/npack.1`.

```text
npack pack <directory> --output <file.npk>
npack init <directory> --name <name> --publisher <npub-or-hex> [--version <semver>] [--os <os>] [--arch <arch>]
npack manifest <file.npk> --output <manifest.json>
npack install <file.npk> [--user|--system|--store <path>]
npack verify <file.npk-or-manifest.json>
npack refresh [--relay <url>] [--pubkey <hex>] [--store <path>]
npack search <query> [--refresh] [--no-cache] [--relay <url>] [--store <path>]
npack info [<publisher>/]<name> [--trusted-publisher <hex>] [--store <path>]
npack install <publisher>/<name> [options]
npack install-ref <publisher>/<name> [options]  # compatibility alias
npack resolve <publisher>/<name> --relay <url> [options]
npack update <publisher>/<name> [options]
npack list [--user|--system]
npack verify-installed [--user|--system]
npack remove <publisher>/<name> [--user|--system]
npack publish <manifest> --secret-key <key> [options]
npack appstream <file.npk> [--output <metainfo.xml>]
npack daemon [--socket <path>] [--system]
```
