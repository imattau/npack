# npack web store

This local web store follows the Soapbox MKStack project structure and uses
React, Vite, and Nostrify. Nostrify discovers kind 9900 release events;
package search, trust state, verification, and installation remain delegated to
the npack daemon.

Build the browser bundle from this directory with `npm ci && npm run build`.
The resulting `dist/` files are embedded in the Rust daemon, so rebuild the
frontend before rebuilding `npack` when changing the UI.

Run the user daemon with `npack daemon --web` and open the loopback URL it
prints. The API is served from the same origin and uses a random, per-process
bearer token passed in the launch URL fragment. The page saves it in tab-scoped
session storage and removes the fragment immediately; fragments are not sent
in HTTP requests. Fetching the root page does not reveal the token. The system
daemon does not serve HTTP.

The same-origin API is intentionally small:

- `POST /api/rpc` accepts the daemon JSON-RPC shape (`id`, `method`, `params`).
- `POST /api/local/inspect` accepts the raw `.npk` bytes and returns the
  inspected preview plus an opaque upload id.
- `POST /api/local/install` consumes that upload id; `/api/local/discard`
  removes a preview the user cancelled.

Every API call requires the bearer token and an exact local `Host`; browser
requests with an `Origin` must match the served origin. The API is not a remote
management interface.
