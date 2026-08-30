# npack

An independent, Nostr-native package manager.

npack is intended to manage the complete package lifecycle: discover signed release metadata on Nostr, resolve dependencies, retrieve content-addressed artifacts, verify them, and install them into an npack-managed store.

## First vertical slice

The current prototype works entirely locally. It defines a package manifest, calculates SHA-256 hashes, verifies an artifact, checks declared dependencies, installs it into a versioned store, and records installed packages.

    cargo run -- hash ./hello.tar.gz
    cargo run -- verify ./hello.npack.json
    cargo run -- install ./hello.npack.json --store /tmp/npack-store
    cargo run -- list --store /tmp/npack-store
    cargo run -- release-event ./hello.npack.json --secret-key <32-byte-hex-key>
    cargo run -- verify-event ./release.json ./hello.npack.json
    cargo run -- search hello --relay wss://relay.example
    cargo run -- fetch <sha256> --server https://blossom.example --output ./artifact
    cargo run -- install-ref <publisher-hex>/hello --relay wss://relay.example --store /tmp/npack-store

Example manifest:

    {
      "publisher": "npub1example",
      "name": "hello",
      "version": "0.1.0",
      "artifact": "hello.tar.gz",
      "sha256": "<64 lowercase hexadecimal characters>",
      "dependencies": []
    }

The manifest is deliberately format-neutral. The release-event command uses the official Rust Nostr library for event IDs, tags, key handling, and Schnorr signatures while preserving this local package lifecycle.

The release-event command emits a signed, provisional kind:9900 Nostr package-release event. The verify-event command validates the event signature and checks it against the package manifest. The search command queries configured relays and displays only cryptographically valid release events. The fetch command retrieves a hash-addressed blob through nostr-blossom and verifies its SHA-256 before writing it. The install-ref command connects these pieces for a verified remote release, recursively installing dependencies before dependents and printing the resulting install order. Publisher-qualified references constrain selection to a specific event author.
