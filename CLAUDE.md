# npack

npack is an independent package manager whose registry metadata will be published on Nostr and whose artifacts may be stored on Blossom. It owns package discovery, trust, dependency resolution, verification, installation, upgrades, and removal.

## Current scope

- Rust CLI, terminal-first.
- Local package manifests and .npk artifacts are the first vertical slice.
- SHA-256 verification is mandatory before installation.
- Installation is into an npack-managed store; native package managers are not required.
- Relay-backed release discovery, Blossom retrieval, recursive dependency-first remote installation, declared dependency validation, and signed release-event generation are implemented with official crates. More advanced solving and publisher selection remain planned.

## Commands

    npack hash <artifact>
    npack verify <manifest>
    npack install <manifest> [--store <path>]
    npack list [--store <path>]
    npack release-event <manifest> --secret-key <hex-key>
    npack verify-event <event> <manifest>
    npack search <query> --relay <relay-url>
    npack fetch <sha256> --server <blossom-url> --output <path>
    npack install-ref [<publisher-hex>/]<name> --relay <relay-url> [--store <path>]
    npack pack <source-directory> --output <package.npk>

## Conventions

- Keep package identity publisher-addressed: publisher/name/version.
- Never trust a mirror URL without checking the declared SHA-256.
- Prefer small, testable domain types over CLI-specific logic.
- Run cargo fmt --check and cargo test before committing.
- Provisional release events use kind 9900 and Nostr Schnorr signatures over the canonical event array.
- The canonical artifact format is .npk, a tar archive compressed with zstd; Nostr event metadata remains authoritative.
