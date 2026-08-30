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

Search configured relays:

```bash
npack search myapp --relay wss://relay.example
npack search myapp --refresh   # ignore the five-minute cache
npack search myapp --no-cache  # do not read or write the cache
```

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

To check every package in the selected install store for a newer release:

```bash
npack update --user
npack update --system
```

The global update command uses the installed publisher/name and requires a
strictly newer version, then reuses normal dependency resolution, artifact
hash verification, and install ordering. A targeted update remains available
with `npack update <publisher>/package`.

Search displays progress on stderr while querying relays. Successful search
results are cached locally for five minutes. The cache key includes the query,
relay set, trusted-publisher filter, and NIP-65 identity, keeping results from
different trust configurations separate. Results are then reduced to the
newest valid SemVer release for each publisher/package pair, while retaining
all platform artifacts belonging to that release.

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
explicitly.

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
npack search <query> [--relay <url>]
npack install <publisher>/<name> [options]
npack install-ref <publisher>/<name> [options]  # compatibility alias
npack update <publisher>/<name> [options]
npack list [--user|--system]
npack verify-installed [--user|--system]
npack remove <publisher>/<name> [--user|--system]
npack publish <manifest> --secret-key <key> [options]
```
