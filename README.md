# npack

An independent, Nostr-native package manager.

npack is intended to manage the complete package lifecycle: discover signed release metadata on Nostr, resolve dependencies, retrieve content-addressed artifacts, verify them, and install them into an npack-managed store.

## First vertical slice

The current prototype works entirely locally. It defines a package manifest, calculates SHA-256 hashes, verifies an artifact, installs it into a versioned store, and records installed packages.

    cargo run -- hash ./hello.tar.gz
    cargo run -- verify ./hello.npack.json
    cargo run -- install ./hello.npack.json --store /tmp/npack-store
    cargo run -- list --store /tmp/npack-store

Example manifest:

    {
      "publisher": "npub1example",
      "name": "hello",
      "version": "0.1.0",
      "artifact": "hello.tar.gz",
      "sha256": "<64 lowercase hexadecimal characters>",
      "dependencies": []
    }

The manifest is deliberately format-neutral. Signature and Nostr event fields will be added while preserving this local package lifecycle.
