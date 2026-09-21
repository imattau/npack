# Nix integration test: `npack resolve` -> `pkgs.fetchurl`

This records an end-to-end test of the workflow requested in
[issue #1](https://github.com/imattau/npack/issues/1): a declarative package
manager (Nix) resolving and fetching a real npack release using only
`npack resolve`'s JSON output, with Nix's own fetcher doing the SHA-256
verification instead of npack.

The test ran against npack's own real, signed release on Nostr/Blossom (the
same release process every `npack` version ships through), not a mock. It
used a disposable `podman` container with Nix installed; nothing from this
test persists on the host that ran it.

## 1. Resolve the release

```bash
npack resolve 483440dfc4efeb8776ca27413c929653ea60484ca2d0e6b97678a0622c6bf413/npack \
  --relay wss://relay.damus.io --relay wss://nos.lol
```

```json
{
  "publisher": "483440dfc4efeb8776ca27413c929653ea60484ca2d0e6b97678a0622c6bf413",
  "name": "npack",
  "version": "0.2.12",
  "sha256": "0d75fd591bc45b8e21ebff4f0de05a4ae32d297dd3e6eb4eac6eabb1749d70e5",
  "os": "linux",
  "arch": "x86_64",
  "format": "npk",
  "artifact_urls": [
    "https://blossom.primal.net/0d75fd591bc45b8e21ebff4f0de05a4ae32d297dd3e6eb4eac6eabb1749d70e5"
  ],
  "dependencies": [],
  "conflicts": [],
  "runtime_requires": [
    "libgcc_s.so.1",
    "libm.so.6",
    "libc.so.6",
    "ld-linux-x86-64.so.2"
  ],
  "provides": [],
  "release_event_id": "76709197ff0bbe4adca14238c18bcf6c2cc16034eeed29292061038f9d29683f",
  "artifact_event_id": "064938892cea36923d4d527c37c477cb6f351e0bff7c1ec03f8e3773abd03091",
  "verification": {
    "release_signature_valid": true,
    "artifact_event_signature_valid": true,
    "release_event_is_v1": true,
    "publisher_trusted": true,
    "revoked": false
  }
}
```

npack did the relay discovery, trust/revocation checks, and Nostr signature
verification. Everything below uses only the `sha256` and `artifact_urls`
fields from this output; Nix never talks to a relay.

## 2. Build a derivation from that output

```nix
{ pkgs ? import (builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/refs/tags/24.05.tar.gz";
  }) {}
}:

# Built entirely from the `npack resolve` output above. npack did the
# discovery/trust/signature verification; Nix's own fetchurl does the
# SHA-256 verification and owns the store path -- the split of
# responsibilities requested in npack issue #1.
pkgs.stdenv.mkDerivation {
  pname = "npack";
  version = "0.2.12";

  src = pkgs.fetchurl {
    url = "https://blossom.primal.net/0d75fd591bc45b8e21ebff4f0de05a4ae32d297dd3e6eb4eac6eabb1749d70e5";
    sha256 = "0d75fd591bc45b8e21ebff4f0de05a4ae32d297dd3e6eb4eac6eabb1749d70e5";
  };

  nativeBuildInputs = [ pkgs.zstd pkgs.autoPatchelfHook ];
  buildInputs = [ pkgs.stdenv.cc.cc.lib pkgs.glibc ];

  # npack's canonical artifact format: a tar archive compressed with zstd.
  unpackPhase = ''
    tar --use-compress-program=unzstd -xf $src
  '';

  dontBuild = true;

  installPhase = ''
    mkdir -p $out/bin $out/share/man/man1
    cp bin/npack $out/bin/npack
    chmod +x $out/bin/npack
    if [ -f share/man/man1/npack.1 ]; then
      cp share/man/man1/npack.1 $out/share/man/man1/npack.1
    fi
  '';

  # runtime_requires from the resolved release (libgcc_s.so.1, libm.so.6,
  # libc.so.6, ld-linux-x86-64.so.2) are satisfied by autoPatchelfHook
  # rewriting the interpreter/rpath to the Nix store instead of the host's
  # /lib64 -- the CI-built binary otherwise targets a generic glibc host,
  # not a NixOS one.
}
```

```bash
nix-build npack-from-resolve.nix -o result
```

Nix fetched the artifact from the resolved URL, verified its own copy of the
SHA-256 (independent of npack), unpacked the zstd-compressed tar archive,
and `autoPatchelfHook` patched the ELF interpreter and RPATH:

```
auto-patchelf: 0 dependencies could not be satisfied
/nix/store/1aw2l55yshxyky3zvj2l33i64p2gl9c2-npack-0.2.12
```

## 3. Run the Nix-built binary

```bash
$ result/bin/npack --version
npack 0.2.12
```

The binary that Nix fetched, verified, and patched runs correctly and
reports the version `npack resolve` said it would.

## 4. Negative control: a tampered hash is rejected

Repeating the build with the `sha256` changed to an incorrect value:

```
error: hash mismatch in fixed-output derivation '/nix/store/.../<sha>.drv':
         specified: sha256-////WRvEW44h6/9PDeBaSuMtKX3T5utOrG6rsXSdcOU=
            got:    sha256-DXX9WRvEW44h6/9PDeBaSuMtKX3T5utOrG6rsXSdcOU=
```

Nix refuses to build. The SHA-256 `npack resolve` reports is load-bearing,
not decorative -- if a mirror were ever compromised or a hash mistyped, the
Nix build fails closed rather than silently accepting different bytes.

## Conclusion

`npack resolve`'s JSON output is directly consumable by a real Nix
derivation with no changes to npack: `sha256` and `artifact_urls` are
sufficient for `pkgs.fetchurl`, and `runtime_requires` correctly predicts
what `autoPatchelfHook` needs to patch. This closes the loop on the
NixOS-friendly consumption model described in
[the roadmap](roadmap.md#declarative-and-immutable-os-integration) and
requested in issue #1, using npack's real production release rather than a
synthetic fixture.

Remaining follow-up, per the roadmap, is validation against a full NixOS
system closure (a flake-based `nixosModules`/package entry rather than a
one-off derivation) and doing the same walk with `npack resolve --recursive`
against a package that actually declares dependencies.
