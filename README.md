# npack

An independent, Nostr-native package manager.

npack is intended to manage the complete package lifecycle: discover signed release metadata on Nostr, resolve dependencies, retrieve content-addressed artifacts, verify them, and install them into the host filesystem.

## First vertical slice

The current prototype works locally or through configured Nostr relays and Blossom servers. It defines a package manifest, calculates SHA-256 hashes, verifies an artifact, checks declared dependencies and conflicts, installs it into a host prefix, and records installed packages.

System installation is the default and uses `/` as the payload prefix with package state in `/var/lib/npack`, so it normally requires privilege. Use `--user` to install payloads into `$HOME/.local`; user state remains in the user's local data directory. `--system` explicitly selects the system scope when configuration defaults to user scope. `--store` is available as an explicit development/test state and prefix override.

Persistent defaults can be configured in `$XDG_CONFIG_HOME/npack/config.toml` (or the platform config directory):

    [network]
    relays = ["wss://relay.example"]

    [storage]
    blossom = ["https://blossom.example"]

    [identity]
    pubkey = "npub1..."

    [trust]
    publishers = ["npub1..."]

    [install]
    user = true

Command-line values take precedence over configuration values.
When `identity.pubkey` is configured, npack reads the user's NIP-65 kind:10002 relay-list event from the configured relays and adds its read-capable relays to package discovery. The configured relays act as bootstrap relays. Configured Blossom servers are added as artifact fallbacks after URLs advertised by the NIP-94 event.

    cargo run -- hash ./hello.tar.gz
    cargo run -- verify ./hello.npack.json
    cargo run -- install ./hello.npack.json --store /tmp/npack-store
    cargo run -- install ./hello.npack.json --user
    cargo run -- list --store /tmp/npack-store
    cargo run -- verify-installed --store /tmp/npack-store
    cargo run -- release-event ./hello.npack.json --secret-key <32-byte-hex-key>
    cargo run -- verify-event ./release.json ./hello.npack.json
    cargo run -- revoke-event ./release.json --secret-key <32-byte-hex-key> --reason "security issue"
    cargo run -- search hello --relay wss://relay.example
    cargo run -- search hello --relay wss://relay.example --trusted-publisher <publisher-hex>
    cargo run -- fetch <sha256> --server https://blossom.example --output ./artifact
    cargo run -- fetch <sha256> --output ./artifact
    cargo run -- publish ./hello.npack.json --secret-key <32-byte-hex-key> --relay wss://relay.example --server https://blossom.example
    cargo run -- install-ref <publisher-hex>/hello --relay wss://relay.example --user
    cargo run -- update <publisher-hex>/hello --relay wss://relay.example --user
    cargo run -- update hello --relay wss://relay.example --trusted-publisher <publisher-hex>
    cargo run -- update hello --relay wss://relay.example --lockfile ./npack.lock.json
    cargo run -- update hello --relay wss://relay.example --lockfile ./npack.lock.json --locked
    cargo run -- update hello --lockfile ./npack.lock.json --locked --offline
    cargo run -- pack ./package-root --output ./hello-1.0.0.npk
    cargo run -- remove <publisher-hex>/hello --store /tmp/npack-store
    cargo run -- inspect ./package-root/bin/hello

Example manifest:

    {
      "publisher": "npub1example",
      "name": "hello",
      "version": "0.1.0",
      "artifact": "hello.tar.gz",
      "sha256": "<64 lowercase hexadecimal characters>",
      "dependencies": [],
      "conflicts": [],
      "runtime_requires": ["libc.so.6"],
      "provides": []
    }

The canonical transport artifact is .npk: a tar archive compressed with zstd. The manifest is deliberately format-neutral. dependencies declare required packages and conflicts declare packages that cannot coexist. runtime_requires records discovered runtime capabilities such as ELF DT_NEEDED libraries, while provides records capabilities supplied by a package. Exact capabilities use names such as libc.so.6; versioned capabilities use forms such as libfoo-api >=2.0.0 and libfoo-api@2.4.1. Installation checks runtime_requires against installed provides or the host system’s OS, architecture, and standard shared-library directories before extraction. Declarative post_install actions support create-directory inside the selected install prefix and register-service for the selected systemd scope. Service registration requires --allow-capability service-manager and does not enable or start the service. Remote installation filters releases to the current OS and architecture, while any remains portable; among otherwise valid releases it selects the highest version, then uses event time and event ID as deterministic tie-breakers. The release-event command uses the official Rust Nostr library for event IDs, tags, key handling, and Schnorr signatures while preserving this local package lifecycle.

The release-event command emits a signed v1 kind:9900 Nostr package-release event. The verify-event command validates the event signature and complete metadata against the package manifest. The revoke-event command emits a publisher-signed v1 kind:9901 revocation event; remote installation rejects releases revoked by their publisher. The search command queries configured relays and displays only cryptographically valid v1 release events. The fetch command retrieves a hash-addressed blob through nostr-blossom and verifies its SHA-256 before writing it. The pack command creates .npk archives, and installation safely extracts them into the selected host prefix. The inspect command reads ELF dependency metadata without executing the artifact. The verify-installed command audits recorded artifact hashes and installed payload paths. The install-ref command connects these pieces for a verified remote release, recursively installing dependencies before dependents and printing the resulting install order; `--lockfile` records the selected package versions, hashes, dependency and conflict declarations, runtime requirements, and provides in install order, while `--locked` requires the complete dependency graph, exact values, package set, and order on replay. The remove command deletes all installed versions for a publisher-qualified package reference. Publisher-qualified references constrain selection to a specific event author.

The publish command uploads a verified `.npk` through `nostr-blossom`, creates and publishes a signed NIP-94 `kind:1063` artifact event, then publishes the signed package-release event through `nostr-sdk`. It uses configured relays plus NIP-65 write relays when `--pubkey` is supplied. Remote installation also consults the publisher's verified Blossom `kind:10063` server list before configured storage fallbacks.

Offline locked replay is available with `--locked --offline`. Online installs cache the verified v1 release and NIP-94 events under the package state directory and reuse the verified artifact cache; offline replay requires those cache entries and never queries relays or Blossom servers. Delegated/offline release keys remain a separate future extension.
