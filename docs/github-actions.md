# GitHub Actions release pipeline

The repository includes a tag-driven reference workflow at
`.github/workflows/release.yml`. It treats GitHub Actions as a builder and
provenance source, while keeping the package protocol independent of GitHub.

For a `v<version>` tag, the workflow:

1. Builds `npack` in release mode.
2. Creates a deterministic Linux x86-64 `.npk` archive.
3. Inspects the built executable's ELF `DT_NEEDED` entries and writes them as
   `runtime_requires` in the package manifest.
4. Writes a package manifest and SHA-256 checksum file.
5. Generates an SPDX SBOM.
6. Creates a GitHub build-provenance attestation for the `.npk`.
7. Uploads the bundle as a GitHub Release and workflow artifact.

Set the repository variable `NOSTR_PUBLISHER` to the publisher's public key.
Configure the protected `release` environment with the secret
`NOSTR_SECRET_KEY`, containing a dedicated package-publisher private key. Add
the repository variables `NOSTR_RELAYS` and `NOSTR_BLOSSOM_SERVERS`, one URL
per line. The tag-only publish job downloads the attested bundle and invokes
`npack publish` with those values.

This first implementation uses GitHub Secrets because it is the simplest
working deployment. The private key is available to the isolated release job
only; it is not exposed to pull requests or printed in logs. Use a dedicated
publisher key rather than a personal primary identity. A future NIP-46 or
OIDC-backed signer can replace this job without changing the package format.

The resulting trust claims remain separate:

- GitHub attestation: this exact artifact came from this repository/workflow
  and commit.
- Nostr signature: the publisher authorizes this package release.
- SHA-256: every Blossom, GitHub, or other transport copy has identical bytes.

The workflow is intentionally a reference implementation rather than a
protocol dependency. Other builders may produce the same `.npk`, manifest,
and event shape from GitLab, Forgejo, a local build, or a reproducible-build
service.
