# npack

`npack` is a Nostr-native package manager. It distributes signed software
release metadata over Nostr relays and stores immutable `.npk` artifacts on
Blossom-compatible servers.

The package format is independent of APT, RPM, and native package databases.
An `.npk` is a deterministic tar archive compressed with zstd. `npack`
resolves dependencies, verifies signatures and SHA-256 hashes, and installs
files directly into the host system or the user's standard local prefix.

## Status

This is an early working prototype. The local package lifecycle, Nostr
release events, NIP-94 artifact metadata, Blossom discovery, dependency
resolution, lockfiles, revocations, offline replay, and GitHub Actions release
workflow are implemented. The wire format is a project protocol and is not
yet a registered NIP.

## How it works

```text
Nostr relays       signed package metadata and discovery
      │
      ▼
kind:9900 release ─── kind:1063 NIP-94 artifact metadata
      │                              │
      └──────── SHA-256 ─────────────┘
                                     │
                                     ▼
                         Blossom / content-addressed storage
```

The publisher's Nostr key identifies the release. Relays and download URLs
are transport; clients verify the event signatures and artifact hash locally.

See:

- [Using npack](docs/using-npack.md)
- [Package protocol v1](docs/package-protocol-v1.md)
- [Event fixtures](docs/package-event-fixtures.md)
- [Release-key notes](docs/release-key-model.md)
- [GitHub Actions release workflow](docs/github-actions.md)

## Build

```bash
cargo build --release
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## Bootstrap installation

The first `npack` package is `npack` itself. A bootstrap binary can be built
from source or downloaded from a GitHub Release. That binary installs future
`.npk` releases, including updates to itself.

System installation is the default and targets the host filesystem. It
normally requires privilege and stores package state in `/var/lib/npack`.
Use `--user` for the host's user-local prefix (`$HOME/.local`) and user-local
state. Use `--store` for isolated development or test installations.

## Common commands

```bash
# Build and inspect an artifact
npack init ./myapp --name myapp --version 1.0.0 --publisher npub1...
npack pack ./myapp --output ./myapp-1.0.0.npk
npack manifest ./myapp-1.0.0.npk --output ./myapp-1.0.0.manifest.json
npack hash ./myapp-1.0.0.npk
npack inspect ./myapp-1.0.0.npk

# Install and inspect local packages
# The package metadata is embedded in the .npk.
npack install ./myapp-1.0.0.npk --user
npack list --user
npack verify-installed --user
npack remove <publisher>/myapp --user

# A standalone manifest remains useful for publishing and verification
npack verify ./myapp.manifest.json

# Create and verify signed release metadata
npack release-event ./myapp.manifest.json --secret-key <secret-key>
npack verify-event ./release.json ./myapp.manifest.json
npack revoke-event ./release.json --secret-key <secret-key> \
  --reason "security issue"

# Discover and install from Nostr
npack search myapp --relay wss://relay.example
npack install <publisher>/myapp --relay wss://relay.example --user
npack update <publisher>/myapp --relay wss://relay.example --user

# Publish an artifact and its Nostr events
npack publish ./myapp.manifest.json \
  --secret-key <secret-key> \
  --relay wss://relay.example \
  --server https://blossom.example
```

Publisher keys can be supplied as user-facing `npub` values where a public
key is accepted; internal comparisons use canonical hex keys.

## Configuration

Configuration is read from `npack/config.toml` in the platform's user config
directory, normally `$XDG_CONFIG_HOME/npack/config.toml`:

```toml
[network]
relays = ["wss://relay.example"]

[storage]
blossom = ["https://blossom.example"]

[identity]
pubkey = "npub1..."

[trust]
publishers = ["npub1..."]

[install]
user = false
```

When an identity is configured, `npack` reads the user's NIP-65 relay list
and adds read-capable relays to discovery. For artifact retrieval it also
checks the publisher's Blossom `kind:10063` server list before configured
fallback servers.

## Security model

- Release events are publisher-signed Nostr `kind:9900` events.
- Artifact metadata uses signed NIP-94 `kind:1063` events.
- Artifact bytes are accepted only when their SHA-256 matches the release.
- Revocations are signed `kind:9901` events and remain effective if relays
  retain copies of the original release.
- Dependencies and runtime capabilities are declared in signed metadata.
- Post-install actions are declarative and capability-gated; services are
  installed but not enabled or started automatically.
- GitHub Actions uses a dedicated `NOSTR_SECRET_KEY` in a protected release
  environment for the current publication prototype. Do not use a personal
  primary Nostr identity key.

## GitHub Releases

The tag-driven workflow builds the first `npack` `.npk` package, extracts ELF
runtime requirements, generates SHA-256 and SPDX metadata, creates a GitHub
provenance attestation, and publishes the artifact and Nostr events when the
protected release environment is configured.

Required repository configuration is documented in
[docs/github-actions.md](docs/github-actions.md).

## License

MIT
