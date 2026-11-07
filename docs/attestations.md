# User attestations and web of trust

This is a proposed post-v1 extension. Nothing here is implemented, and protocol
v1 clients ignore it. It is written to be reviewed and changed before any code.

## Why

Today npack can tell you who published a release and that the bytes match what
they signed. It cannot tell you whether anyone you know has used it, looked at
it or is worried about it. A publisher you have never heard of is shown as
"Not trusted yet", and the only way to resolve that is to decide alone.

People already publish follow lists on Nostr. If people can also say, in a
signed event, "I vouch for these exact bytes" or "I would not install this",
a client can show the opinions of the people the user already chooses to
listen to, and nobody else's.

## Principles

1. **Advice, never authority.** An attestation can inform a decision. It never
   installs anything, never replaces the signature and SHA-256 checks, and never
   marks a publisher as trusted. Trusting a publisher stays an explicit user
   action (`npack trust add`, or the confirmation dialog).
2. **Only your own circle counts.** Anyone can create thousands of keys. An
   attestation from a key outside the user's circle is ignored, not discounted.
3. **No score.** Clients show who vouches and who warns, by name. A single
   number invites gaming and hides the reasoning.
4. **Exact bytes.** An attestation is bound to a release event and its SHA-256,
   not to a name, so it cannot be carried over to a different upload.
5. **Opt-in and previewed.** A public attestation reveals what software a
   person uses. Nothing is published without the user seeing the exact event.
6. **Keys stay safe.** Reading attestations needs only a public key. Writing
   needs a signature, which is done by a remote signer or the OS keyring, never
   by a secret key pasted into the GUI.

## Two different things called "attestation"

- **Build provenance** (machine): GitHub's attestation that a `.npk` came from
  a given repository, workflow and commit. The release workflow already
  creates it. npack does not read or verify it today.
- **User attestation** (human): this document. A person's endorsement or
  warning about a release.

They are separate claims and a client should never merge them into one badge.
Recording a pointer to the build provenance in the release event is possible
but is a separate proposal.

## The attestation event

A provisional kind `9903` event (`9902` is taken by the release-key proposal),
signed by the attestor. It is a regular, non-replaceable event.

```json
{
  "kind": 9903,
  "pubkey": "<attestor>",
  "tags": [
    ["v", "1"],
    ["e", "<release-event-id>"],
    ["p", "<publisher-pubkey>"],
    ["name", "notepad-lite"],
    ["version", "1.0.0"],
    ["x", "<sha256-of-the-artifact>"],
    ["verdict", "reviewed"],
    ["how", "built-from-source"]
  ],
  "content": "Built it from the tagged commit, no surprises."
}
```

Required tags: `v`, `e`, `p`, `name`, `version`, `x`, `verdict`. Each must appear
once. `content` is optional plain text of at most 280 characters.

### Verdicts

| Verdict | Meaning |
| --- | --- |
| `use` | I run this and it has behaved as described. |
| `reviewed` | I looked at what it does (code, package contents or behaviour). |
| `warn` | I would not install this. The reason belongs in `content`. |
| `retract` | I withdraw my earlier attestation for this release. |

`how` is an optional, repeatable hint for `reviewed`: `built-from-source`,
`read-the-code`, `ran-in-sandbox`. Unknown values are shown as plain text and
never change behaviour.

### Which event counts

For each attestor and each `x`, only the newest event (by `created_at`, then
event id) counts. A `retract` as the newest event means that attestor has no
current attestation. This is the same "newest wins" rule used for republished
listings.

### Validation (clients)

An event is accepted only if all of these hold, otherwise it is dropped
silently (never an error, so a hostile event cannot break browsing):

- the signature is valid, `v` is `1`, and required singleton tags are present
  once each;
- `x` is 64 hex characters, and `e`, `p`, `name`, `version`, `x` agree with a
  release the client actually knows (from the catalogue or a fetched event).
  An attestation for a release the client cannot find is kept but not shown;
- `content` is cleaned like any other listing text: control and bidirectional
  override characters removed, length capped.

## Whose attestations count

The client builds an **attestor set** from, in this order:

1. **Explicit attestors:** `[trust].attestors` in `config.toml`, managed like
   trusted publishers (`npack trust attestor add|remove|list`, with the same
   confirmation rules and NIP-05 name support).
2. **Follow circle:** the follow list (kind 3, NIP-02) of
   `[identity].pubkey`, to a configured depth. Depth 1 (people the user
   follows) is the default. Depth 2 is opt-in and capped at a fixed number of
   keys so one popular account cannot pull in the whole network.
3. **Mutes:** a key on the user's mute list (NIP-51, kind 10000) is removed
   from the set even if it is otherwise reachable.

Where the set came from is part of what the client shows ("you follow Alice",
"a friend of someone you follow"), because the reason a person is listened to
matters as much as what they said.

Reading follow and mute lists is a relay query that names the user's public
key. npack already does this for NIP-65 relay lists, so it is not a new
disclosure, but the settings screen should say so.

## What clients do with it

- **Browse rows and details:** one line such as "Vouched for by Alice and 2
  others you follow". Names are the verified NIP-05 label if there is one, an
  "(unverified)" claimed name otherwise, then a short npub, as elsewhere.
- **Confirmation dialog for an untrusted publisher:** the same line, plus any
  notes, and an offer to trust the publisher only if the user chooses. Nothing
  changes the default button.
- **Warnings:** a `warn` from anyone in the set is shown above the Install
  button in a warning colour with the attestor's name and reason. It does not
  block installing: the user may know better. It is also never shown as if the
  publisher had said it.
- **Revoked releases:** a revocation by the publisher (kind 9901) always
  outranks attestations; the client shows the revocation instead.
- **Updates:** attestations are per release, so a new version starts with none.
  The client may say "Alice vouched for 1.0.0, not yet for 1.1.0".
- **CLI:** `npack info` and `npack search --long` print the same lines, and
  `npack resolve` adds them to its JSON under `attestations`.

## Writing an attestation

`npack attest <publisher>/<name>[@version] --verdict use|reviewed|warn
[--how <hint>] [--note <text>]` and a button on an installed app. Both:

1. resolve the release exactly as `npack update` would (trust, signature,
   artifact event and hash all verified), so nobody attests to something that
   would not install;
2. show the exact event that will be published, and the relays it will go to,
   and ask for consent (`--yes` for scripts, as with `npack trust add`);
3. sign and publish.

**Signing.** Two ways, in order of preference:

- **Remote signer (NIP-46):** the user's own signer app approves each event.
  npack never sees the secret key. This is the default for the GUI.
- **OS keyring:** the existing `register_secret_key` path used for publisher
  keys, for people who want it. The key is never printed or logged.

The signing key belongs to a person, so attestations are signed by the user's
own daemon or CLI process, never by the system daemon (`npack daemon --system`),
which must never hold a user's key. The system daemon only reads attestations.

## Daemon API (additive)

| Method | Params | Result |
| --- | --- | --- |
| `GetAttestations` | `releases[]` (release event ids), `refresh` | Per release: `vouched[]` and `warned[]`, each `{attestor, npub, label, verdict, how[], note, reason_in_set}`, plus counts. Only attestors in the set. |
| `ListAttestors` | none | The explicit attestors and a summary of the follow circle (size, depth, age). |
| `AddAttestor` / `RemoveAttestor` | `attestor` (hex, npub or `name@domain`) | Like the trusted-publisher methods, gated on the system daemon by `io.npack.configure`. |
| `Attest` | `package`, `verdict`, `how[]`, `note`, `dry_run` | With `dry_run`, the event that would be published. Otherwise publishes. User daemon only. |

`Search` results and `GetPackage` gain an `attestations` summary (counts and the
first few names) so a browse list needs no extra call per row. New error codes:
`no_identity` (nothing to read a circle from), `signer_unavailable`,
`attest_refused` (the release would not install).

## Threats and what answers them

| Threat | Answer |
| --- | --- |
| Fake accounts vouch for malware | Only keys in the user's own circle count; others are ignored. |
| A friend's key is stolen and used to vouch | Attestations are advice shown with the name; installs still need signature and hash checks and, for new publishers, the confirmation dialog. The user can mute the key. |
| A rival posts `warn` to damage a publisher | A warning counts only if the user listens to the warner, is labelled as the warner's opinion, and never blocks. Only the publisher can revoke. |
| An attestation reused for a different upload | Bound to the SHA-256 and release event id. |
| Relay hides or delays attestations | Treated like any other relay data: the absence of an attestation means "none known", never "safe". |
| Social pressure to vouch | Nothing is published without a preview, and there is no count or leaderboard to chase. |
| The attestor set leaks who someone is | Reading uses only the public key the user chose to set, over the relays they already use. The settings screen says so. |
| A huge follow graph slows browsing | Depth and size caps; results are cached and refreshed on demand. |
| Hostile text in notes | Cleaned and capped like every other listing text; shown as plain text, links are not followed. |

## Out of scope

Reputation scores, rankings, payments or bounties, moderation, automatic
trust of any publisher, sharing a user's attestations without their consent,
and verifying GitHub build provenance (a separate proposal).

## Open questions

1. **Own kind or NIP-32 labels?** A dedicated kind is simpler to validate and
   keeps the hash binding explicit. Reusing the existing Nostr label event
   would let other Nostr apps read attestations, at the cost of a looser
   shape. The exact NIP-32 and NIP-85 details should be checked against the
   current specifications before choosing.
2. **Depth 2 by default?** Safer at 1; more useful for small networks.
3. **Should `warn` ever interrupt?** Proposed: no, but a strong warning from
   several people in the set could justify a confirmation step.
4. **Publisher replies.** Should a publisher be able to answer a `warn` in a
   linked event? Useful, but it adds a second event kind and moderation
   questions.

## Plan

1. **Read side:** attestor set (explicit list, follow circle, mutes),
   `GetAttestations`, and the lines in Browse, details, the confirmation dialog
   and `npack info`. Tests use in-memory events; end-to-end uses a local `nak`
   relay as elsewhere.
2. **Write side:** `npack attest`, the GUI button, NIP-46 and keyring signing,
   dry-run preview.
3. **Warnings and retraction** polish, and the settings screen for the circle.

Each step is shippable on its own, additive, and needs no change to the v1
release or artifact events.
