# npack package protocol v1

This document defines the current npack wire format. It is a project
protocol using Nostr custom event kinds; it is not yet a registered NIP.

## Interoperability profile

All events use the ordinary Nostr event envelope from NIP-01. The custom
package event kinds are regular events, so relays may store and replicate
them without special package support. Event IDs and signatures are computed
by the normal Nostr serialization rules; clients must verify both before
interpreting package data.

The examples in [package-event-fixtures.md](package-event-fixtures.md) are
canonical tag-shape fixtures. Values such as keys, hashes, and event IDs must
be replaced with real values and then the complete event must be signed.

## Release events

A package release is a signed Nostr event with kind 9900 and a required
["v", "1"] tag. The event author is the package publisher.

Required singleton tags (exactly one of each):

| Tag | Meaning |
| --- | --- |
| d | name/version/arch release coordinate |
| v | Protocol version, currently 1 |
| name | Package name |
| version | SemVer package version |
| os | Target OS, or any |
| arch | Target architecture, or any |
| format | Artifact format, normally npk |
| x | Lowercase SHA-256 of the artifact |
| artifact | NIP-94 kind 1063 event ID |

The artifact event must be signed by the same publisher and contain the
same SHA-256 x tag. Its URL tags identify candidate download locations.
Clients verify the downloaded bytes against x; URLs and Blossom servers are
transport only.

`x` is exactly 64 lowercase hexadecimal characters. `version` is SemVer.
`d` is `<name>/<version>/<arch>`. The canonical artifact format for v1 is
`npk`, a tar archive compressed with zstd. Package names and tag values that
carry identifiers must not contain path traversal components.

Optional singleton tags are repo and commit, linking the release to a NIP-34
repository and source commit. When present, `repo` must be a valid
`30617:<pubkey>:<identifier>` repository address and `commit` must be a
non-empty commit identifier. Clients may use these values for provenance
display and later repository-state verification; source availability is not
required to install an otherwise valid artifact.

Repeatable tags:

| Tag | Values |
| --- | --- |
| depends | name, requirement, or publisher, name, requirement |
| conflicts | name, requirement, or publisher, name, requirement |
| requires | Runtime capability requirement |
| provides | Runtime capability supplied by the package |
| post-install | action, relative-path |

Unknown tags are ignored for forward compatibility. Required singleton tags
must not be duplicated. Malformed dependency or post-install tags invalidate
the release.

## Revocation events

A revocation is a signed Nostr event with kind 9901, signed by the release
publisher, and containing:

    ["v", "1"]
    ["e", "<release-event-id>"]
    ["name", "<package-name>"]
    ["version", "<semver>"]
    ["x", "<sha256>"]
    ["reason", "<human-readable-reason>"]

The required tags occur exactly once, `e` is a 64-character event ID, `x` is
exactly 64 hexadecimal characters, `version` is SemVer, and `reason` is
non-empty. Clients treat a valid revocation referencing a release event as
authoritative and reject that release. Deleting or replacing the original
event is not required.

## Identity and trust

The event author is the publisher identity. Clients may display it as npub
but should compare keys canonically. Package names are not globally owned;
the stable identity is publisher key plus package name.

Trust lists, relay selection, NIP-65 user relay discovery, and curator
policies are client decisions. NIP-85 reputation is not an installation
security requirement in v1.

## Release keys

Delegated or offline release keys are deliberately not part of v1. Releases
are signed directly by the publisher event-author key. The proposed
authorization event is reserved as a separately versioned extension in
release-key-model.md, pending protocol review.
