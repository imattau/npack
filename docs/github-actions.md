# GitHub Actions release pipeline

The repository includes a tag-driven reference workflow at
`.github/workflows/release.yml`. It treats GitHub Actions as a builder and
provenance source, while keeping the package protocol independent of GitHub.

For a `v<version>` tag, the workflow:

1. Builds `npack` in release mode.
2. Creates a deterministic Linux x86-64 `.npk` archive.
3. Builds an x86-64 Debian package (`.deb`) and RPM package (`.rpm`) with the
   `npack` binary and man page.
4. Inspects the built executable's ELF `DT_NEEDED` entries and writes them as
   `runtime_requires` in the package manifest.
5. Writes a package manifest and SHA-256 checksum file.
6. Generates an SPDX SBOM.
7. Creates a GitHub build-provenance attestation for the `.npk`.
8. Uploads the bundle as a GitHub Release and workflow artifact.

The native packages depend on the system C library and CA certificates. Their
package manager metadata supports routine install and removal; npack's own
`.npk` package format remains the source for Nostr-published releases.

## Cutting a release without a local push

Bump the version in both `Cargo.toml` files and `Cargo.lock`, and merge that to
the default branch. Then run the `Create release` workflow from the Actions tab
on that branch, entering the version (no `v` prefix) and a one-line summary. It
checks the version against both crates (`packaging/check-release-version.sh`),
refuses an existing tag or release, creates the annotated `v<version>` tag, and
starts `Release package` on it. The dispatch is explicit because a tag pushed
with the workflow's own token does not start other workflows.

To rehearse, run `Release package` by hand on the default branch: it builds
everything but skips the GitHub Release and the Nostr job.

To add native packages to an existing GitHub Release, run the
`Publish Linux packages` workflow from the Actions tab and enter the release
version without its `v` prefix. It checks out that release's tag and uploads
the matching `.deb` and `.rpm` files to that release. This workflow does not
publish anything to Nostr.

Set the repository variable `NOSTR_PUBLISHER` to the publisher's public key.
Configure the protected `release` environment with the secret
`NOSTR_SECRET_KEY`, containing a dedicated package-publisher private key. Add
the repository variables `NOSTR_RELAYS` and `NOSTR_BLOSSOM_SERVERS`, one URL
per line. The tag-only publish job downloads the attested bundle and invokes
`npack publish` with those values.

Each artifact event lists the package's GitHub Release asset URL
(`--artifact-url`) as its primary download location, and the Blossom servers as
mirrors. Before signing, the job downloads each asset and refuses to continue
unless it is byte-identical to the package it built. The store icon is not
uploaded to Blossom: it is served from the tagged source on
`raw.githubusercontent.com` (`--icon-url`), checked the same way.

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
