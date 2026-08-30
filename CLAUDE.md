# npack

npack is an independent package manager whose registry metadata will be published on Nostr and whose artifacts may be stored on Blossom. It owns package discovery, trust, dependency resolution, verification, installation, upgrades, and removal.

## Current scope

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
    npack search <query> --relay <relay-url>
    npack fetch <sha256> --server <blossom-url> --output <path>
    npack install-ref [<publisher>/]<name> --relay <relay-url> [--user|--system] [--store <path>] [--lockfile <path>] [--locked] [--allow-capability <capability>]
    npack pack <source-directory> --output <package.npk>
    npack remove <publisher>/<name> [--user|--system] [--store <path>]
    npack inspect <artifact>

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
