# Package setup: implementation plan

## Goal

Let a package publisher declare values required or accepted at install time,
then let the CLI, npackd clients, and external installers such as NostrHost
collect values in their own UI and submit them to npack. Descriptors are
signed release metadata; values are per-install data and never part of the
signed release event. npack owns
the common input contract and validation; platform-specific configuration and
deployment remain the responsibility of platform integrations.

## Responsibilities

- Publishers declare typed install inputs and requirements.
- Installers discover that contract, collect values, present the proposed
  changes, and pass values to npack.
- npack validates values and exposes them to the authorized installer
  transaction. Integrations decide how to use them for platform-specific
  configuration and deployment.

## Delivery sequence

The first slice is implemented: signed descriptors, `GetPackage` discovery,
and daemon `Install.install_values` envelope validation. Extension type
interpretation and platform-specific effects remain with the client or
platform integration.

1. Define a versioned install-input descriptor in the package manifest and
   signed release metadata. The descriptor has a stable field identifier,
   required/optional status, presentation hints, and a type identifier. Core
   types may be standardized over time, but the type system stays open:
   package authors and platforms can define namespaced types and associated
   constraints. Unknown descriptors remain visible to clients and are not
   silently discarded. A client that cannot handle a required type must stop
   before install and explain the limitation. Support sensitive values
   without logging or persisting them.
2. Extend `GetPackage` and `Install` so clients can discover requirements
   and submit per-package values. Keep values out of lockfiles, signed release
   events, package cache metadata, and transaction progress records.
3. Define the validation boundary: npack validates the envelope, field
   identifiers, required values, and any universally understood constraints.
   Type-specific validation belongs to a client or platform integration that
   declares support for that type. The daemon transports values without
   reinterpreting supported extension types. Keep privileged host changes
   behind existing authorization and capability boundaries. The installer,
   not npack core, owns platform-specific preview, application, and rollback.
4. Document NostrHost-style and GUI clients using the same daemon contract,
   with examples of different platform integrations.

## Initial security boundaries

No arbitrary package scripts, shell interpolation, or implicit persistence of
sensitive values. Install inputs are data; they do not grant a package new
host privileges or define a universal configuration templating language.
Types use namespaced identifiers so platforms can evolve their own value
contracts without requiring every type to be built into npack. Do not execute
publisher-provided validators or renderers as part of parsing a release.

## Acceptance criteria

- A frontend can fetch a verified release and render its install form without
  downloading or installing the artifact.
- The same setup values work through the CLI and daemon API.
- Missing values and malformed envelopes fail before host mutation. Clients
  enforce constraints for types they support and refuse required types they
  cannot handle.
- A platform integration can consume the validated values without depending
  on NostrHost-specific fields or prompts.
