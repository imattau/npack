# npack package protocol v1

This document defines the current npack wire format. It is a project
protocol using Nostr custom event kinds; it is not yet a registered NIP.

## Release events

A package release is a signed Nostr event with kind 9900 and a required
["v", "1"] tag. The event author is the package publisher.

Required singleton tags:

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

Optional singleton tags are repo and commit, linking the release to a NIP-34
repository and source commit.

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

Clients treat a valid revocation referencing a release event as authoritative
and reject that release. Deleting or replacing the original event is not
required.

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
