# Package event fixtures

These fixtures define the v1 tag shapes for independent implementations.
They are templates rather than signed events: replace every placeholder,
compute the NIP-01 event ID, and sign the resulting event with the publisher
key.

## NIP-94 artifact event

```json
{
  "kind": 1063,
  "tags": [
    ["url", "https://blossom.example/<sha256>.npk"],
    ["m", "application/zstd"],
    ["x", "<64-lowercase-hex-sha256>"],
    ["size", "<decimal-byte-count>"]
  ],
  "content": "<package-name>"
}
```

The artifact event author is the same publisher as the release event. At
least one `url` tag is required by npack v1. Clients must hash downloaded
bytes and compare them with `x`; a URL is never trusted by itself.

## Package release event

```json
{
  "kind": 9900,
  "tags": [
    ["d", "<name>/<semver>/<arch>"],
    ["v", "1"],
    ["name", "<package-name>"],
    ["version", "<semver>"],
    ["os", "<os-or-any>"],
    ["arch", "<arch-or-any>"],
    ["format", "npk"],
    ["x", "<64-lowercase-hex-sha256>"],
    ["artifact", "<kind-1063-event-id>"],
    ["depends", "<dependency-name>", ">=<semver>"],
    ["conflicts", "<package-name>", "<semver-range>"],
    ["requires", "<runtime-capability>"],
    ["provides", "<runtime-capability>"],
    ["post-install", "<action>", "<relative-path>"],
    ["repo", "30617:<repo-pubkey>:<repo-identifier>"],
    ["commit", "<source-commit-id>"]
  ],
  "content": "<release-notes>"
}
```

`depends`, `conflicts`, `requires`, `provides`, and `post-install` are
repeatable. `repo`, `commit`, and all core release tags are singleton tags.
Unknown tags are ignored so newer optional metadata does not make an older
client reject an otherwise valid release.

## Revocation event

```json
{
  "kind": 9901,
  "tags": [
    ["v", "1"],
    ["e", "<kind-9900-event-id>"],
    ["name", "<package-name>"],
    ["version", "<semver>"],
    ["x", "<64-lowercase-hex-sha256>"],
    ["reason", "<non-empty-reason>"]
  ],
  "content": "<reason>"
}
```

The revocation author must be the release publisher. A client should retain
valid revocations even when the original release event is no longer
available from a relay.
