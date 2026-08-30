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
      "runtime_requires": ["libc.so.6"],
      "provides": []
    }

The canonical transport artifact is .npk: a tar archive compressed with zstd. The manifest is deliberately format-neutral. runtime_requires records discovered runtime capabilities such as ELF DT_NEEDED libraries, while provides records capabilities supplied by a package. Exact capabilities use names such as libc.so.6; versioned capabilities use forms such as libfoo-api >=2.0.0 and libfoo-api@2.4.1. Installation checks runtime_requires against installed provides or the host system’s OS, architecture, and standard shared-library directories before extraction. Remote installation filters releases to the current OS and architecture, while any remains portable. The release-event command uses the official Rust Nostr library for event IDs, tags, key handling, and Schnorr signatures while preserving this local package lifecycle.

The release-event command emits a signed, provisional kind:9900 Nostr package-release event. The verify-event command validates the event signature and checks it against the package manifest. The search command queries configured relays and displays only cryptographically valid release events. The fetch command retrieves a hash-addressed blob through nostr-blossom and verifies its SHA-256 before writing it. The pack command creates .npk archives, and installation safely extracts them into the npack-managed store. The inspect command reads ELF dependency metadata without executing the artifact. The install-ref command connects these pieces for a verified remote release, recursively installing dependencies before dependents and printing the resulting install order. The remove command deletes all installed versions for a publisher-qualified package reference. Publisher-qualified references constrain selection to a specific event author.
