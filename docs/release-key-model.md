# Release-key model

The publisher's identity key should not need to remain online for every package
build or release. The publisher key is therefore treated as an offline root,
while one or more short-lived release keys may sign package releases.

## Identities

- `publisher`: the long-term Nostr identity that owns the package namespace.
- `release key`: an online or CI key authorized by the publisher to sign
  releases for a constrained scope and time window.
- `builder`: an optional key that attests to how an artifact was produced.

## Authorization event

The publisher emits a provisional kind `9902` event:

```json
{
  "kind": 9902,
  "pubkey": "<publisher-pubkey>",
  "tags": [
    ["d", "release-key:<release-pubkey>"],
    ["p", "<release-pubkey>"],
    ["scope", "foo"],
    ["since", "1735689600"],
    ["until", "1767225600"]
  ],
  "content": ""
}
```

The event author is the publisher. `scope` may be a package name or `*`.
`since` and `until` are Unix timestamps. An authorization is valid only when
the release key, package scope, and release timestamp all match.

## Release verification

A release event continues to use kind `9900` and adds:

```text
["publisher", "<publisher-pubkey>"]
```

The release signer is valid when either:

1. it is the publisher key; or
2. it is a release key with a valid kind `9902` authorization from the
   publisher.

The artifact event must have the same publisher and release signer. Clients
must reject releases with missing or ambiguous publisher tags when delegated
signing is used.

## Revocation

A publisher can revoke an authorization by publishing a newer kind `9902`
event for the same release key and scope with a `revoked` tag. A release key
can also be revoked directly by a kind `9901` event signed by the publisher.
Clients must check authorization revocations before accepting a release.

## Security rules

- A release key cannot authorize another release key.
- Authorization scope cannot be broadened by a release key.
- Expired authorizations are invalid even if relays retain the event.
- Relay presence is never evidence of authorization; signatures and event
  relationships must be verified locally.
- Trust or reputation signals may assist discovery but cannot replace these
  cryptographic checks.

This model is provisional until the event kinds and tag vocabulary are
reviewed against existing Nostr conventions.
