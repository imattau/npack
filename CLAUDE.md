# npack

npack is an independent package manager whose registry metadata will be published on Nostr and whose artifacts may be stored on Blossom. It owns package discovery, trust, dependency resolution, verification, installation, upgrades, and removal.

## Current scope

- Rust CLI, terminal-first.
- Local package manifests and artifacts are the first vertical slice.
- SHA-256 verification is mandatory before installation.
- Installation is into an npack-managed store; native package managers are not required.
- Nostr relay, Blossom transport, signatures, and dependency solving are planned interfaces, not yet implemented.

## Commands

    npack hash <artifact>
    npack verify <manifest>
    npack install <manifest> [--store <path>]
    npack list [--store <path>]

## Conventions

- Keep package identity publisher-addressed: publisher/name/version.
- Never trust a mirror URL without checking the declared SHA-256.
- Prefer small, testable domain types over CLI-specific logic.
- Run cargo fmt --check and cargo test before committing.
