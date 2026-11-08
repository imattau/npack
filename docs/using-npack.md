# Using npack

`npack` is a Nostr-native package manager. It discovers signed release
metadata on Nostr relays, downloads immutable `.npk` files from Blossom or
other HTTP-compatible storage, verifies the artifact hash, resolves
dependencies, and installs files using the host operating system's normal
filesystem layout.

The project is an early prototype. The package event format is a project
protocol and is not yet a registered NIP.

## Install npack

### From a release

Each GitHub release has Debian and RPM packages (and `.npk` archives, which
npack itself can install). There are three:

| Package | What it is |
| --- | --- |
| `npack` | The command line tool, the `npackd` service, the man page, the systemd units and the PolicyKit policy. No graphics libraries, so it installs on servers and containers. |
| `npack-gui` | The desktop app, with its launcher entry, icon and AppStream description. Needs the matching `npack`. |
| `npack-desktop` | An empty package that installs the other two. This is the one to pick for a desktop. |

```bash
# Debian, Ubuntu and derivatives: the command line tool only
sudo apt install ./npack_0.5.0_amd64.deb

# ...or the desktop as well (give apt all three files together)
sudo apt install ./npack_0.5.0_amd64.deb ./npack-gui_0.5.0_amd64.deb ./npack-desktop_0.5.0_amd64.deb

# Fedora and other RPM systems
sudo dnf install ./npack-0.5.0-1.x86_64.rpm ./npack-gui-0.5.0-1.x86_64.rpm ./npack-desktop-0.5.0-1.x86_64.rpm
```

Open **npack** from your applications menu. The app starts the background
service itself when it isn't already running, so there is nothing else to set
up. If you would rather have the service start at login for other tools to use:

```bash
systemctl --user enable --now npackd.service
```

The packages declare the libraries the desktop app loads at run time (X11 or
Wayland, EGL and Vulkan). Software rendering works if your machine has no GPU
driver, provided Mesa's drivers are installed (the package recommends them).

### From source

Build from source:

```bash
git clone https://github.com/imattau/npack.git
cd npack
cargo build --release
install -m 0755 target/release/npack ~/.local/bin/npack
```

Check the installation:

```bash
npack --version
```

The first package is `npack` itself. A bootstrap binary can install later
versions of `npack` in the same way as any other package.

## Installation modes

System installation is the default:

```bash
npack install ./myapp-1.0.0.npk
```

This installs into the host filesystem, normally requiring administrator
privileges. Package state is stored in `/var/lib/npack`.

For an unprivileged user-local installation:

```bash
npack install ./myapp-1.0.0.npk --user
```

This uses the host's conventional `~/.local` prefix and user-local npack
state. It does not create a separate application directory or replace the
host's filesystem conventions.

For development and tests, use an isolated store:

```bash
npack install ./myapp-1.0.0.npk --store /tmp/npack-store
```

`--user` and `--system` are mutually exclusive. `--system` is the explicit
form of the default.

## Installing a local `.npk`

An `.npk` is a deterministic tar archive compressed with zstd. A package
contains its local installation metadata at `.npack/manifest.json`:

```text
.npack/manifest.json
bin/myapp
lib/libmyapp.so
share/myapp/README.md
```

Install it directly:

```bash
npack install ./myapp-1.0.0.npk --user
```

`npack` reads the embedded metadata, calculates the archive SHA-256, verifies
the package contents, checks dependencies and conflicts, and records the
installed files. The `.npack` metadata directory is not installed into the
target prefix.

Standalone local installation proves archive integrity, but it does not
prove who published the package. For publisher authentication, install from
Nostr using a signed release event.

## Creating an `.npk`

Start a package scaffold with:

```bash
npack init ./myapp \
  --name myapp \
  --version 1.0.0 \
  --publisher npub1...
```

This creates `.npack/manifest.json` with the host OS and architecture. Use
`--os` and `--arch` to target another platform, for example
`--os linux --arch aarch64`. Add payload files beneath the scaffold, then
pack it:

```bash
npack pack ./myapp --output ./myapp-1.0.0-linux-x86_64.npk
```

Create a package root using normal host paths:

```text
package-root/
├── .npack/
│   └── manifest.json
├── bin/
│   └── myapp
└── share/
    └── myapp/
        └── README.md
```

The embedded manifest describes the package. Its `sha256` is left empty when
building because the archive hash is not known until the archive is complete:

```json
{
  "publisher": "npub1...",
  "name": "myapp",
  "version": "1.0.0",
  "artifact": "myapp-1.0.0.npk",
  "sha256": "",
  "dependencies": [],
  "conflicts": [],
  "os": "linux",
  "arch": "x86_64",
  "format": "npk",
  "runtime_requires": [],
  "provides": [],
  "post_install": []
}
```

Build and inspect the archive:

```bash
npack pack ./package-root --output ./myapp-1.0.0.npk
npack manifest ./myapp-1.0.0.npk \
  --output ./myapp-1.0.0.manifest.json
npack hash ./myapp-1.0.0.npk
npack inspect ./myapp-1.0.0.npk
npack verify ./myapp-1.0.0.npk
```

`npack pack` preserves directory structure, executable permissions and
symlinks. It normalizes timestamps and ownership and sorts entries so the
same package root produces the same archive.

The package root should contain payload paths relative to the host root:
`bin/`, `lib/`, `share/`, `etc/`, and so on. Do not put `/usr` or `/home`
inside the archive unless that is deliberately part of the target layout.

### Hard-coded install paths

The same package is installed under `~/.local` for one person and under `/` for
everyone, so a file that names an absolute location works in one mode and
breaks in the other. `npack pack` (and `npack publish`, which checks the
finished archive) warns about:

- **Your own files by absolute path:** `ExecStart=/usr/bin/app` or
  `DATA=/usr/share/app/data` in a unit, script, `.desktop` or config file, when
  the package installs `bin/app` or `share/app/data` itself. References to
  system tools (`/usr/bin/env`, `/etc/ssl`) are fine.
- **Build-machine paths:** the directory you packed from, `/home/<name>`,
  `/Users/<name>`, `/root`, CI work folders. In binaries only the directory you
  packed from is reported, because compilers record their own toolchain's paths.
- **Absolute library paths:** an ELF `RPATH`/`RUNPATH` outside `/lib`, `/usr/lib`
  and friends. Use `$ORIGIN/../lib`.
- **Absolute symlink targets.**

Documentation, man pages, licences and translations are skipped, as are
`.npack/` files. Nothing in the package is ever run: the check only reads
bytes. Findings are warnings by default; `npack pack --strict` fails (before
writing anything) so a build script can insist. A path that is hard-coded on
purpose is allowed with `--allow-path /usr/share/zoneinfo` (repeatable) or by
listing it, one absolute prefix per line, in `.npack/path-allow.txt` inside the
package.

#### Writing paths that move: `{{prefix}}`

Instead of a fixed path, a package can write `{{prefix}}` where its install
location belongs, in the text files that need it (service files, launchers,
wrapper scripts). List those files in `.npack/templates.txt`, one
package-relative path per line (`#` starts a comment):

```text
# .npack/templates.txt
lib/systemd/user/app.service
share/applications/app.desktop
```

```ini
# lib/systemd/user/app.service
[Service]
ExecStart={{prefix}}/bin/app --serve
```

When the package is installed, npack replaces `{{prefix}}` in exactly the listed
files with the prefix it installs under: `/home/me/.local` for a user install, so
the line becomes `ExecStart=/home/me/.local/bin/app --serve`, and nothing for an
install at `/`, so it becomes `ExecStart=/bin/app --serve` (lay the package out
the way you want it to land under `/`). Only `{{prefix}}` is expanded: no
environment, no commands, and files that are not listed are installed exactly as
they are. The list is inside the archive, so the same signed hash covers it, and
the stored archive stays untouched, so a rollback or reinstall fills in the
placeholder again for wherever it lands.

`npack pack` checks the list: a listed file that is missing, a link, too big
(over 1 MiB) or not text is an error, and a file that uses `{{prefix}}` without
being listed (or is listed without using it) is a warning, so the placeholder
never reaches a user unexpanded. An install refuses the same problems before it
writes anything.

## Manifest fields

The important fields are:

| Field | Purpose |
| --- | --- |
| `publisher` | Nostr public key identifying the publisher; use an `npub` for user-facing configuration. |
| `name` | Package name within the publisher namespace. |
| `version` | Semantic version. |
| `artifact` | Archive filename. |
| `sha256` | Final archive hash in a published manifest; empty only in embedded build metadata. |
| `dependencies` | Other npack packages and version requirements. |
| `conflicts` | Packages that cannot coexist with this package. |
| `os`, `arch` | Compatibility target, such as `linux` and `x86_64`, or `any`. |
| `runtime_requires` | Host capabilities needed by the package, such as ELF libraries. |
| `provides` | Capabilities supplied to other packages. |
| `post_install` | Declarative, capability-gated installation actions. |
| `app` | Optional desktop-store metadata: `summary`, `description`, `homepage`, `license`, `categories`, `icon`, `screenshots`, `desktop_file`, `release_date`, `release_notes`, `support_url`. See [AppStream metadata](#appstream-metadata) below. |

Packages may also set `app.install_inputs` to an array of install-time input
descriptors. Each has an identifier, a namespaced `value_type`, `required`,
`sensitive`, optional `description`, and opaque JSON `constraints`. Type
identifiers are open-ended so packages and platform integrations can define
their own. npack signs and returns descriptors as release metadata; clients
collect values and send them per install. npack checks required fields and
rejects undeclared values, but does not interpret extension types or persist
the supplied values.

Two more optional top-level fields, `repo` and `commit`, link a release to
its source. `commit` is any non-empty commit identifier. `repo` must be a
NIP-34 `30617:<pubkey>:<identifier>` repository address, **not** a plain
URL - `npack pack`/`npack publish` reject anything else with "manifest repo
must be a NIP-34 kind:30617 address". Neither field has a dedicated CLI
flag yet; set them by editing `repo`/`commit` directly in
`.npack/manifest.json` after `npack init`, before `npack pack`. See
[Release events](package-protocol-v1.md#release-events) for the full tag
format - clients may use these for provenance display and repository-state
verification, but source availability is never required to install an
otherwise valid artifact.

An external publishing manifest must contain the final SHA-256:

```bash
sha256=$(npack hash ./myapp-1.0.0.npk)
```

The external manifest is used by the publishing and verification commands;
end users normally receive equivalent release metadata from Nostr.

Generate an external release manifest from a completed archive instead of copying the embedded
metadata by hand:

```bash
npack manifest ./myapp-1.0.0.npk \
  --output ./myapp-1.0.0.manifest.json
```

## AppStream metadata

A manifest's optional `app` object describes the package in the language
Linux application stores already understand:

```json
{
  "publisher": "npub1...",
  "name": "myapp",
  "version": "1.0.0",
  "artifact": "myapp-1.0.0.npk",
  "sha256": "",
  "app": {
    "summary": "A friendly greeting",
    "description": "Prints a friendly greeting to the terminal.",
    "homepage": "https://example.com/myapp",
    "license": "MIT",
    "categories": ["Utility"],
    "icon": "share/icons/myapp.png",
    "screenshots": ["https://example.com/myapp/screenshot.png"],
    "desktop_file": "myapp.desktop",
    "release_date": "2026-01-15",
    "release_notes": "Fixes a crash on start.\nFaster search.",
    "support_url": "https://example.com/myapp/help"
  }
}
```

`release_notes` (at most 2000 characters) is the "what's new" for this version
and `support_url` an http(s) page for help; both are published with the
release so a store can show them without downloading the package. The package
size and installed size are not written by hand: `npack publish` measures the
artifact it uploads and the files inside it and adds them to the release event.
`npack publish` also warns when a release has no summary or categories, because
a store cannot describe it or list it under a category (publish again with an
`app` section to fix it; the newer event replaces the older one).

`icon` and `desktop_file` are package-relative paths to files included in the
`.npk`; `npack pack` checks both exist in the source directory, and validates
`desktop_file` as a syntactically correct freedesktop.org
[Desktop Entry](https://specifications.freedesktop.org/desktop-entry-spec/latest/)
file (a `[Desktop Entry]` group with `Type` and `Name`, plus `Exec` when
`Type=Application`).

`npack inspect` prints the declared icon path with the rest of the embedded
metadata. `npack appstream` references the icon as an icon-theme (stock) name
derived from the file stem; a package that declares `app.desktop_file` but no
`app.icon` gets a warning on stderr, because the generated document would
silently lack an `<icon>` element. `npack init` prints a reminder that
`app.icon` is where a package logo is declared.

Generate an [AppStream](https://www.freedesktop.org/software/appstream/docs/)
component document from a built archive:

```bash
npack appstream ./myapp-1.0.0.npk --output ./myapp.metainfo.xml
```

Packages without a `desktop_file` are rendered as a `console-application`
component advertising `<provides><binary>myapp</binary></provides>`; packages
with one are rendered as a `desktop-application` component advertising
`<launchable type="desktop-id">myapp.desktop</launchable>`. Component IDs are
namespaced `io.npack.<publisher>.<name>`, since npack publishers are Nostr
public keys rather than domains. Validate the generated document with
[`appstreamcli`](https://www.freedesktop.org/software/appstream/docs/man/appstreamcli.1.html):

```bash
appstreamcli validate --no-net ./myapp.metainfo.xml
```

## Dependencies and install order

Dependencies use package names and semantic version requirements:

```json
"dependencies": [
  { "name": "libfoo", "requirement": ">=2.0.0" },
  { "publisher": "npub1...", "name": "helper", "requirement": "^1.4" }
]
```

The publisher-qualified form is preferred when a name could be ambiguous.
The resolver:

1. Finds a compatible signed release.
2. Resolves its dependencies recursively.
3. Detects cycles and conflicts.
4. Installs dependencies before the requested package.
5. Prints and records the resulting order.

Write a lockfile for repeatable deployments:

```bash
npack install npub1.../myapp \
  --relay wss://relay.example \
  --lockfile npack.lock
```

Replay the exact locked graph offline after artifacts and metadata have been
cached:

```bash
npack update npub1.../myapp \
  --lockfile npack.lock \
  --locked \
  --offline
```

## Discovering and installing from Nostr

Build the local catalogue once, then browse it with no further relay
round trips:

```bash
npack refresh --relay wss://relay.example
npack search myapp
npack info myapp
```

`npack search` reads the catalogue file (default `<npack state dir>/
catalogue.json`, or `--store <path>` for an isolated one) by default and
never touches a relay unless asked:

```bash
npack search myapp --relay wss://relay.example --refresh   # rebuild the catalogue from relays, then search it
npack search myapp --relay wss://relay.example --no-cache  # query relays live for this search only, without touching the catalogue
```

If no catalogue exists yet, plain `npack search` fails with a message
pointing at `npack refresh` rather than silently querying relays.

#### Browsing by category

Releases that publish a store listing can be browsed by category:

```bash
npack categories                                  # "Utility (12)", most populous first
npack search "" --category utility --sort name    # an empty query matches everything
npack search "" --sort recent --limit 20          # newest releases first
```

`npack categories` lists the categories in the catalogue with the number of
packages in each, counting only releases that fit this machine (add
`--all-platforms` to include the rest). Category names are matched without
regard to case, and spelling variants are one category, shown in their most
common spelling. `--sort recent` orders by when the release event was
published, which the publisher sets, so treat it as a convenience rather than
a guarantee. Search also matches a query against each package's summary and
categories, not just its name.

Packages whose release lists no category (older releases, or ones published
without an `app` section) are not hidden: `npack categories` ends with an
"(uncategorised)" line and `npack search "" --uncategorised` lists them.

Install a publisher-qualified package:

```bash
npack install npub1.../myapp \
  --relay wss://relay.example \
  --user
```

`install-ref` remains accepted as a compatibility alias for scripts using the
older command name. The remote installation process is:

1. Query relays for signed `kind:9900` release events.
2. Verify the Nostr signature and release fields.
3. Reject revoked or incompatible releases.
4. Resolve dependencies and determine install order.
5. Fetch the signed NIP-94 `kind:1063` artifact metadata.
6. Try the publisher's Blossom server list and configured fallback servers.
7. Verify the downloaded bytes against the release SHA-256.
8. Install the verified `.npk`.

Relays and storage servers are transport. They do not become package
authorities merely because they served an event or file.

Each step prints progress to stderr (connecting, resolving, downloading) so
the process doesn't look hung during slow relay or Blossom lookups, and each
installed package prints a confirmation to stdout on success:

```text
Connecting to 1 relay(s)... done in 0.4s
Resolving npub1.../myapp...
Downloading npub1.../myapp 1.0.0 (2 mirror(s))... done in 1.1s (48213 bytes)
installed npub1.../myapp 1.0.0
install order: npub1.../myapp
```

To check every package in the selected install store for a newer release:

```bash
npack update --user
npack update --system
```

The global update command uses the installed publisher/name and requires a
strictly newer version, then reuses normal dependency resolution, artifact
hash verification, and install ordering. A targeted update remains available
with `npack update <publisher>/package`.

Pass `--check` to report available updates without installing anything, the
`apt update` equivalent to `update`'s `apt upgrade`:

```bash
npack update --user --check
npack update <publisher>/myapp --relay wss://relay.example --check
```

For each installed package this resolves and verifies the newest release
matching a strictly newer version, the same way `update` does, but stops
before downloading the artifact or installing it. Output looks like:

```text
npub1.../myapp 1.0.0 -> 1.1.0 available
npub1.../otherapp 2.3.0 up to date
1 update(s) available.
```

`--check` also reports installed packages whose release has been **revoked** by
their publisher, with the publisher's reason:

```text
[REVOKED] npub1.../myapp 1.0.0: signing key leaked
```

A revocation counts only when it is a valid signed event from the package's own
publisher naming that exact name, version and artifact hash. It is a signal to
update or remove the package, not something npack acts on by itself.

#### Automatic checks and desktop notifications

`npack update --check --notify` does the same check and then sends a desktop
notification through `notify-send` (install `libnotify`/`libnotify-bin`) when
something has appeared that it has not already told you about: a new update, or
a revoked install (which is sent as critical urgency). It remembers what it
announced in `update-notified.txt` in the store, so running it from a timer does
not repeat itself until the situation changes. If `notify-send` is missing the
command fails rather than staying silent.

To check daily, enable the timer. The `npack` package installs the units; from
a source build, copy them from `packaging/` first:

```bash
# from a source build only:
mkdir -p ~/.config/systemd/user
cp packaging/npack-update-check.service packaging/npack-update-check.timer ~/.config/systemd/user/

systemctl --user enable --now npack-update-check.timer
```

The service runs `/usr/bin/npack update --check --notify --user`; edit the path or
flags in the copied unit if npack is installed elsewhere or you use a system
store. The check is randomised by up to two hours and a missed run happens soon
after the machine next starts. The GUI also checks once when it opens, and warns
on every tab while an installed package is revoked.

Search results are reduced to the newest valid SemVer release for each
publisher/package pair, while retaining all platform artifacts belonging to
that release; a `--trusted-publisher` filter and NIP-65 identity narrow which
catalogue entries are considered, same as before.

### `npack info`: a package's full picture from the catalogue

```bash
npack info myapp
npack info npub1.../myapp
```

Unlike `search`, `info` has no `--refresh`/`--no-cache` escape hatch to
relays -- it always reads the local catalogue, since showing every known
version/publisher/platform combination for one package is exactly what the
catalogue is for. Output lists each publisher (optionally annotated
`(trusted)`/`(not in trusted-publisher list)` when `--trusted-publisher` is
given), then every version for that publisher newest first, its os/arch, its
release event id, and `[REVOKED]` when applicable:

```text
myapp
  publisher npub1... (trusted)
    1.2.0 linux/x86_64 76709197...
    1.1.0 linux/x86_64 895abc98... [REVOKED]
```

`npack info` does not print store-listing fields. They are published as release
tags (see [the protocol](package-protocol-v1.md)) and returned by the daemon's
`Search` and `GetPackage`; `npack search` also matches them.

## Resolving release metadata for other package managers

`npack resolve` performs the same relay discovery, trust and revocation
checks, and signature verification as remote installation (steps 1-3 and 5
above), but stops before downloading the artifact or installing anything:

```bash
npack resolve npub1.../myapp --relay wss://relay.example
```

The command prints the resolved publisher, name, version, SHA-256, candidate
artifact URLs (NIP-94 `url` tags plus the publisher's declared Blossom
servers), declared dependencies, and a verification summary as JSON on
stdout, for example:

```json
{
  "publisher": "...",
  "name": "myapp",
  "version": "1.0.0",
  "sha256": "...",
  "os": "linux",
  "arch": "x86_64",
  "format": "npk",
  "artifact_urls": ["https://blossom.example/..."],
  "dependencies": [],
  "conflicts": [],
  "runtime_requires": [],
  "provides": [],
  "release_event_id": "...",
  "artifact_event_id": "...",
  "verification": {
    "release_signature_valid": true,
    "artifact_event_signature_valid": true,
    "release_event_is_v1": true,
    "publisher_trusted": true,
    "revoked": false
  }
}
```

This is intended for declarative package managers (such as Nix) that want to
own fetching, unpacking, and rollback of the artifact themselves while
leaving Nostr/Blossom discovery and trust verification to npack. `--relay`,
`--requirement`, `--trusted-publisher`, `--pubkey`, `--lockfile`, and
`--locked` behave the same as their `install-ref` counterparts. See
[the Nix integration test](nixos-integration-test.md) for a worked example
building a real Nix derivation from this output.

Pass `--recursive` to resolve the full dependency closure in one call instead
of a single package. The command then walks each declared dependency the same
way `install-ref` would, but only resolves and verifies metadata rather than
installing, and prints a JSON array of resolved entries -- one per
publisher/package in the graph -- instead of a single object:

```bash
npack resolve npub1.../myapp --relay wss://relay.example --recursive
```

Dependency cycles are rejected the same way they are during installation, and
a package required at incompatible versions by two different entries in the
graph fails resolution rather than silently picking one. This is the shape a
derivation generator would want to build one `fetchurl`-plus-unpack
derivation per package in the graph, without needing a separate `npack
resolve` invocation for every dependency.

Pass `--release <event-id>` to pin resolution to one exact release event
instead of picking the newest matching version. The event must still be
signed, v1, unrevoked, from a trusted publisher, and match the name and
host os/arch; otherwise resolution fails rather than falling back. It cannot
be combined with `--requirement`, `--lockfile`, or `--recursive`.

The JSON output includes `kind` (`null` for ordinary packages) and
`fork_of` (`null` unless the release declares provenance).

### Nix recipes

A Nix recipe is a `.npk` (`os`/`arch` `any`) with `"kind": "nix-recipe"` that
contains a `default.nix`, a `callPackage`-style function. `npack
install`/`update` refuse these artifacts; Nix consumes them through the pins
described below. Recipes are found like any package; `npack search <text>
--kind nix-recipe` lists only recipes, and `search`/`info` show `[nix-recipe]`
and `(fork of ...)`.

**Publish a recipe**

```bash
npack init myrecipe --name myapp --publisher <npub> --kind nix-recipe
# edits: myrecipe/default.nix (a template is created)
npack pack myrecipe --output myapp-0.1.0.npk
# fill the manifest's sha256, then: npack publish <manifest> --relay ... --server ...
```

**Fork someone else's recipe.** One command verifies and extracts the
published recipe and writes a manifest for your copy that keeps the kind and
records `fork_of`; nothing else needs to be specified:

```bash
npack unpack <publisher>/<name> --output myfork --fork-as <your-npub> [--name <new-name>]
# edit myfork/default.nix, bump the version in myfork/.npack/manifest.json
npack pack myfork --output myfork-1.0.1.npk   # then publish as above
```

Without `--fork-as`, `unpack` only extracts the verified recipe, which is how to
review a recipe's code before using it. `--release <event-id>` unpacks an
exact release.

**Use recipes from Nix** with [`packaging/nix`](../packaging/nix). Add it as a
flake input (`inputs.npack.url = "github:imattau/npack?dir=packaging/nix"`) or
copy `lib.nix` and `npack-nix-pin`.

```bash
# Resolve recursively (dependencies are pinned under their own name) and write pins
packaging/nix/npack-nix-pin --out npack-pins.json \
  myapp=<publisher>/myapp [other=<publisher>/other@<release-event-id>]
# Later: show what would change (exit 1 if anything), then apply
packaging/nix/npack-nix-pin --out npack-pins.json --check myapp=<publisher>/myapp
packaging/nix/npack-nix-pin --out npack-pins.json myapp=<publisher>/myapp
```

Relays and trusted publishers come from npack's config; override them after
`--` (`-- --relay wss://relay.example --trusted-publisher <hex>`). The tool
prints new and changed pins (version and publisher) so a pin update can be
reviewed like any other lockfile change.

```nix
# NixOS configuration (flake)
environment.systemPackages =
  let recipes = inputs.npack.lib.${pkgs.system}.fromPinsWith {
    pinsFile = ./npack-pins.json;
    trustedPublishers = [ "<hex pubkey>" ];  # evaluation fails for any other publisher
  };
  in [ recipes.myapp ];
# without a flake: (import ./lib.nix { inherit pkgs; }).fromPins ./npack-pins.json
```

Every pinned recipe is built in one scope, so a recipe can take its declared
npack dependency as a function argument by name (`{ stdenv, other }: ...`).

**Trust.** A recipe is Nix code that your machine evaluates and then builds,
and evaluating it uses import-from-derivation. The signature and SHA-256 prove
who published the bytes, not that the recipe is safe. Use
`trustedPublishers`/`--trusted-publisher`, review recipes with `npack unpack`,
and pin exact releases for anything important. A dependency declared without a
publisher resolves to whichever trusted publisher has a package of that name,
so recipes should declare the dependency's publisher; `npack-nix-pin` warns when
a dependency comes from a different publisher than its dependent and refuses to
silently replace an existing pin with one from another publisher.

## Running npackd (local service API)

`npack daemon` runs npackd, a common local backend that a GUI store or other
tool can talk to without needing to understand Nostr, Blossom, or `.npk`
internals -- the CLI itself is one possible client:

```bash
npack daemon --socket $XDG_RUNTIME_DIR/npackd.sock
```

`--socket` defaults to `$XDG_RUNTIME_DIR/npackd.sock`. The protocol is
newline-delimited JSON over that Unix socket: one JSON object request per
line, one JSON object response per line, matched by `id`.

```text
--> {"id": 1, "method": "ListInstalled", "params": {"user": true}}
<-- {"id": 1, "result": [{"publisher": "npub1...", "name": "myapp", "version": "1.0.0", ...}]}

--> {"id": 2, "method": "GetPackage", "params": {"package": "npub1.../myapp", "relay": ["wss://relay.example"]}}
<-- {"id": 2, "result": {"publisher": "...", "name": "myapp", "version": "1.2.0", "sha256": "...", ...}}
```

Supported methods, mirroring the CLI operations above:

| Method | Params | Result |
| --- | --- | --- |
| `Search` | `query` (empty matches everything), `relay[]`, `trusted_publisher[]`, `pubkey`, `refresh`, `no_cache`, `include_untrusted`, `category`, `sort` (`name` or `recent`), `limit`, `offset`, `only_trusted`, `host_only` | Array of matching releases, each with `trust` (`"trusted"` or `"unknown"`), `npub`, `published_at` and `compatible` (whether its os/arch fit this machine). With `include_untrusted` the trust list does not filter results. `only_trusted` keeps only publishers in the daemon's trust list (an empty list trusts no one); `host_only` keeps only compatible releases; `category` is case-insensitive. Filters apply before `sort`, `offset` and `limit`. |
| `ListCategories` | the same filters as `Search` (`category`, `sort`, `limit` and `offset` are ignored) | Array of `{category, count}`, most populous first, over the same set a browse view draws from. When some packages list no category (and there is at least one real category), the last entry is `{category: "Uncategorised", count, uncategorised: true}`; list those with `Search` and `"uncategorised": true`. |
| `CatalogueStatus` | `store` (optional) | Read-only, no network: `{present, created_at, age_seconds, stale, stale_after_seconds, releases}` (only `present`, `stale` and `stale_after_seconds` when there is no catalogue). `stale` is true once the catalogue is more than 6 hours old. |
| `RefreshCatalogue` | `relay[]`, `pubkey`, `store` (all optional) | Downloads the catalogue from the relays, saves it and returns the new `CatalogueStatus`. Callers asking at the same moment share one download, and a catalogue fetched in the last 30 seconds is returned as it is. If the relays return no releases at all while a real catalogue exists, the old one is kept and the call fails with `relay_unreachable`, because unreachable relays look like an empty answer. Not gated by PolicyKit, like `Search` with `refresh`. |
| `GetPackage` | `package`, `relay[]`, `requirement`, `os`, `arch`, `trusted_publisher[]`, `store`, `user`, `include_untrusted` | The same resolved-metadata object as `npack resolve`, plus `trust` and `npub`; `verification.publisher_trusted` reflects the trust list even when it is empty. |
| `ListInstalled` | `user`, `store` | Array of installed packages. |
| `Install` | `package`, `requirement`, `relay[]`, `server[]`, `user`, `store`, `allow_capability[]`, `trust_once[]`, `install_values{}`, `async` | The installed package's record, or `{"transaction_id": N}` if `async` is true. `install_values` is a per-install object keyed by descriptor id. |
| `InspectLocal` | `path`, `user`, `store` | Read-only preview of a local `.npk` file: `{name, version, publisher, npub, signed: false, sha256, size, file_name, compatible, dependencies, runtime_requires, post_install, required_capabilities, app, blocked_by}`. Per-user daemon only (the system daemon refuses with `unsupported_here`, since it would read arbitrary paths as root). A file without `.npack/manifest.json` gives `not_a_package`. |
| `InstallLocal` | `path`, `allow_capability[]`, `user`, `store`, `async` | Installs a local `.npk` exactly as `npack install ./file.npk` does (manifest verified, dependencies must already be installed). A local file carries no publisher signature. Per-user daemon only. |
| `Remove` | `package`, `user`, `store` | `{"removed": "<package>"}`. |
| `Update` | `package` (omit for all), `relay[]`, `server[]`, `user`, `store`, `allow_capability[]`, `async` | Array of per-package update outcomes, or `{"transaction_id": N}` if `async` is true. |
| `CheckUpdates` | `package` (omit for all), `relay[]`, `trusted_publisher[]`, `user`, `store` | Array of `{reference, current_version, available_version}`, plus `revoked` (the publisher's reason) when the installed release has been revoked. |
| `GetTransaction` | `transaction_id` | `{"status": "running", "progress": {...}}`, `{"status": "succeeded", "result": ...}`, `{"status": "failed", "error": "..."}`, or `{"status": "cancelled"}`. |
| `CancelTransaction` | `transaction_id` | `{"cancel_requested": true}`. |
| `GetConfig` | none | `{first_run, config_path, relays[], blossom[], relays_defaulted, blossom_defaulted, trusted_publishers[{publisher, npub}]}`. `first_run` is true when no `config.toml` exists yet. |
| `ListTrustedPublishers` | none | Array of `{publisher, npub}`. |
| `AddTrustedPublisher` | `publisher` (npub, hex, or a NIP-05 `name@domain`) | `{publisher, npub}` (plus `nip05`/`label` when a name was given); the resolved key is written to `[trust].publishers` in `config.toml`. |
| `RunDoctor` | `offline`, `user`, `store` | `{healthy, findings[{level, area, title, detail?, fix?}]}`: the same checks as `npack doctor`, with `level` one of `ok`, `info`, `warn`, `fail` and `healthy` false only if some finding is `fail`. Read-only, so not gated by PolicyKit. It can take up to about eight seconds (the relay and download checks); `offline` skips them. See "Health check" below. |
| `GetHistory` | `package`, `limit`, `user`, `store` | Array of `{at, action, publisher, name, from?, to?, reference}`, newest first. |
| `ListRollbacks` | `user`, `store` | Array of `{reference, current_version, previous_version}` for packages that can be switched back right now (archive and metadata verified). |
| `Rollback` | `package`, `allow_capability[]`, `user`, `store` | `{package, from_version, to_version}`. Gated like `Update` on the system daemon. |
| `GetIcon` | `release_event_id`, `store` | `{sha256, media_type, data}` where `data` is the base64 icon (PNG or SVG, at most 256 KiB) of a release in the local catalogue. See "Store icons" below. |
| `ResolvePublisher` | `query` (npub, hex, or `name@domain`), `relay[]` | A publisher identity (below). Use it to show who a typed value is before trusting them. |
| `GetPublisherProfiles` | `publishers[]` (hex or npub), `relay[]`, `refresh` | One identity per input, in order. Best effort: unreachable relays or domains give a bare identity, not an error. Cached for an hour (successful lookups only). |
| `RemoveTrustedPublisher` | `publisher` | `{removed, publisher}`. |
| `SaveConfig` | `relays[]`, `blossom[]` (both optional) | The new `GetConfig` result. Omitted fields keep their effective value (so `{}` saves the built-in defaults and ends first run); an empty list reverts that setting to the built-in defaults. Relays must be `ws://`/`wss://`, servers `http://`/`https://`. |

`Search` results and `GetPackage` include an `app` object when the release
published a store listing: `summary`, `description`, `homepage`, `license`,
`categories[]`, `screenshots[]`, `release_date`, `release_notes`, `support_url`,
`download_size`, `installed_size` (bytes, the publisher's statement), and
`icon {url, sha256}`. It has
been cleaned and bounded by npack, but it is still publisher-supplied text:
show it as text, and do not fetch `screenshots`, `homepage` or `support_url`
without a user action. Results also carry `details` when the release declares
anything a person should see before installing: `repo`, `commit`, `depends[]`,
`requires[]` and `post_install[]` (what installing does besides unpacking
files, such as `register-service lib/systemd/user/app.service`). Only the
newest event for each publisher, name, version, os and arch is returned, so a
republished listing replaces the old one. `Search` also matches a query against the summary and categories.

An unknown method or a request that fails to deserialize its params returns
`{"id": ..., "error": "...", "error_code": "..."}` instead of `result`. Each
connection is handled concurrently.

**Error codes.** `error` is the full human text; `error_code` is a stable
category a frontend can map to its own wording: `unknown_method`,
`invalid_params`, `invalid_request`, `invalid_input` (a value such as a
publisher key or relay URL was rejected), `nip05_failed`, `not_authorized`, `no_relays`,
`no_blossom_servers`, `relay_unreachable`, `store_busy`, `publisher_untrusted`,
`download_failed`, `cancelled`, `no_catalogue` (nothing to browse until the catalogue has been fetched: call `RefreshCatalogue`, or `Search` with `refresh`), `not_found`, `no_icon`, `no_rollback`, `rollback_blocked`, and `internal` for anything else.
A `failed` `GetTransaction` status carries `error_code` the same way. The
field is additive; older daemons omit it.

**Health check.** `RunDoctor` is read-only and open to every local user, so the
daemon runs one check at a time and reuses a finished result for three seconds,
which absorbs a burst of identical requests without making "Check again" stale.
The checks that hash installed archives run on a blocking thread so they do not
hold up other requests.

**Store icons.** `GetIcon` fetches the icon named by a release already in the
daemon's local catalogue; a caller cannot supply a URL, so it is not a general
fetch service. The fetch must be `https` to a public domain name (no IP
literals, ports or local names), follows no redirects, is capped at 256 KiB,
must match the SHA-256 the publisher signed, and must be a PNG or SVG by its
bytes. Verified icons are cached under `<store>/icons/<sha256>` and re-checked
against their hash each time they are served. Releases published to a local
`http://` Blossom server (for example `nak serve`) therefore show no icon in a
GUI; that is deliberate. Error code `no_icon` means the release has none.

**Publisher names (NIP-05).** A publisher is a key; a name like
`alice@example.com` is only a readable pointer to one, so the key is always what
is stored and trusted. An identity is `{publisher, npub, label?, nip05?,
nip05_verified, claimed_name?}`:

- `label` is present only when the publisher's profile (kind 0) claims a NIP-05
  name **and** that domain's `/.well-known/nostr.json` maps the name back to the
  same key. It is the one name a frontend may show as verified.
- `nip05` and `claimed_name` are self-reported by the profile and are not
  verified: anyone can claim any name. Frontends must label them as such.
- Typing a `name@domain` into `ResolvePublisher`/`AddTrustedPublisher` resolves
  it through the domain, which vouches for the key at that moment.

Lookups are HTTPS-only, never follow redirects (as NIP-05 requires), time out
after 10 seconds, cap the response at 64 KiB, and refuse IP literals, ports and
local-only domains (`localhost`, `.local`, `.internal`, `.lan`, ...). A failed
lookup returns `error_code` `nip05_failed`. Lookups tell the domain's operator
that someone resolved that name, and tell relays which profiles were requested.
They are read-only and not PolicyKit-gated, so on the system daemon any local
user can make it fetch a public domain's `nostr.json`.

**Zero-config defaults.** When `[network].relays` or `[storage].blossom` is
empty or the file is missing, npack uses built-in public relays and Blossom
servers (`DEFAULT_RELAYS` / `DEFAULT_BLOSSOM_SERVERS` in `npack-cli`) instead
of failing, for the CLI and the daemon alike. They are only written to
`config.toml` if the user saves settings. No publisher is trusted by default.

**Trust and `include_untrusted`.** On the CLI, an empty `[trust].publishers`
list accepts any publisher and a non-empty list hides all others. The daemon
keeps that behaviour for scripts, and adds `include_untrusted` plus a per-result
`trust` field so a GUI can show publishers the user has not trusted yet and ask
first: "Trust and install" (`AddTrustedPublisher`, then `Install`) or "Install
once" (`Install` with `trust_once`). The confirmation is a frontend policy; the
daemon's enforcement is still the configured list.

By default `Install` and `Update` run to completion before responding. Pass
`"async": true` to get `{"transaction_id": N}` back immediately and poll
`GetTransaction` for progress and the final result:

```text
--> {"id": 1, "method": "Install", "params": {"package": "npub1.../myapp", "relay": ["wss://relay.example"], "async": true}}
<-- {"id": 1, "result": {"transaction_id": 1}}

--> {"id": 2, "method": "GetTransaction", "params": {"transaction_id": 1}}
<-- {"id": 2, "result": {"status": "running", "progress": {"stage": "resolving", "package": "npub1.../myapp"}}}
   ... later ...
--> {"id": 3, "method": "GetTransaction", "params": {"transaction_id": 1}}
<-- {"id": 3, "result": {"status": "running", "progress": {"stage": "downloading", "package": "npub1.../myapp", "detail": "3 mirror(s)"}}}
   ... later ...
--> {"id": 4, "method": "GetTransaction", "params": {"transaction_id": 1}}
<-- {"id": 4, "result": {"status": "succeeded", "result": {"publisher": "...", "name": "myapp", "version": "1.0.0", ...}}}
```

`progress.stage` while `running` is one of `connecting`, `resolving`,
`downloading`, `updating` (during `Update`), or `installed`, each paired with
the publisher/name of the package currently being processed. This is
per-package status, not a per-byte download progress bar -- it is updated at
the same checkpoints `CancelTransaction` is checked at (see below), not on
every downloaded chunk.

`CancelTransaction` is cooperative, not forcible: it is only checked between
packages (before starting the next package in a dependency graph, or the
next package in an `Update` loop), never mid-download or mid-install of a
package already in progress. This means a package that has already started
installing will finish before cancellation takes effect -- the store can
never be left half-installed by a cancelled transaction.

### npackd-user vs. npackd-system, and PolicyKit

Plain `npack daemon` is npackd-user: it runs as your own account and serves
only your own `--user`/default store (`~/.local`) -- there is nothing to gate
since it never acts on anyone else's behalf.

```bash
npack daemon --system
```

`npack daemon --system` is npackd-system: a privileged instance intended to
run as a systemd system service (`packaging/npackd-system.service`), always
as root, serving the system store (`/`, state in `/var/lib/npack`). It
refuses to start unless it is actually running as root. Because its socket
(`/run/npackd.sock` by default) is left world-connectable, it authorizes
each request itself rather than relying on socket permissions: a root peer
is trusted outright, but a non-root peer's `Install`, `Remove`, or `Update`
is checked against PolicyKit before it runs, using the peer's real
credentials read off the socket (`SO_PEERCRED`), not anything the client
claims. This is the same model PackageKit and other system package
services use.

The four actions -- `io.npack.install`, `io.npack.remove`, `io.npack.update`
and `io.npack.configure` (which gates `AddTrustedPublisher`,
`RemoveTrustedPublisher` and `SaveConfig`, since they change what root will
install) -- are defined in [`packaging/io.npack.policy`](../packaging/io.npack.policy),
which a system package installs to `/usr/share/polkit-1/actions/`. They
default to `auth_admin`, so a non-root caller normally sees their desktop's
usual "authenticate as an administrator" prompt (handled by whatever
PolicyKit agent their session runs) the first time they ask npackd-system to
install, remove, or update something. `GetTransaction`, `CancelTransaction`,
and the read-only methods are never gated.

### Reference GUI

`npack-gui` (its own crate in this workspace) is a small `egui`/`eframe`
desktop app that exercises this API -- Search, Details, Install, Installed,
Updates, Remove -- purely as a client of the protocol documented above, with
no dependency on `npack-cli`'s internals:

```bash
cargo run --release -p npack-gui
```

If nothing is listening on the socket, the app starts `npack daemon` for you
(found next to the app's own executable, otherwise on `PATH`) in its own
process group, so closing the window does not stop it. It only ever does this
when nothing is listening: `npack daemon` replaces an existing socket, so a
second one would orphan the first. If it cannot start the service it says so and
offers "Try again".

It reads and writes `$XDG_RUNTIME_DIR/npackd.sock` by default. To talk to a
different instance, change the socket path under Settings, Advanced.

A daemon started this way (or by plain `npack daemon`) is npackd-user, so it
installs into your own prefix (`~/.local`), never the system one.

**Installing for everyone.** The app installs for you alone by default. If the
system-wide service (`npack daemon --system`, usually `npackd-system.service`
started by an administrator) is running, Settings gains an "Install software
for" choice with "Everyone on this computer". Choosing it points the app at
`/run/npackd.sock`; every install, update and removal then asks for an
administrator password through PolicyKit, and a blue strip says so on every tab.
The two services are separate: each has its own installed list, updates,
catalogue, trusted publishers and relays. The choice is remembered, but the app
never starts the system-wide service (it needs root) and falls back to "just me"
when it stops running. The option is hidden while that service is not running.
To set it up: `sudo systemctl enable --now npackd-system`.

Browse shows each app's stated download size, an "Uncategorised" entry for
releases that list no category, and, in the details pane and the install
confirmation, what the release says installing does (for example "adds a
background service"), what it needs on your system, where the source is, the
publisher's release notes and a support link. Updates shows the release notes
and download size of the new version. All of this comes from the signed
release event, so nothing is downloaded to show it; sizes and notes are the
publisher's statements.

The software list (the catalogue) is kept up to date quietly: when the app
starts and the list is more than 6 hours old, or the first time it starts after
the app itself was updated, it downloads a fresh one in the background. The old
list stays on screen until the new one is in, "Software list updated 3 hours
ago" under the Browse heading says how fresh it is, and if the download fails
(offline, relays down) the old list is kept and a note says so, with no error
banner. A window left open looks again every 30 minutes. "Refresh catalogue"
still refreshes on demand. `npack search` reads the same saved list and says how
old it is, and suggests `npack refresh` once it is more than 6 hours old.

Keyboard shortcuts: Ctrl+1 to Ctrl+5 switch tabs, Ctrl+F jumps to the search
box (results update as you type, a moment after you pause), F5 or Ctrl+R
refreshes the current tab, and Escape closes the details pane. The app
follows the system light or dark setting and remembers the last tab and the
window size in `~/.config/npack/gui-state.json`; delete that file to reset
them.

Text from publishers is shown with system fonts as fallbacks (Noto Sans, Noto
CJK, DejaVu Sans, when installed), so other scripts display instead of empty
boxes. egui does not do right-to-left layout yet, so Hebrew and Arabic text
appears in the wrong order.

## Configuration

Configuration is stored at the platform's user config path, normally:

```text
$XDG_CONFIG_HOME/npack/config.toml
```

Example:

```toml
[network]
relays = [
  "wss://relay.example",
  "wss://relay2.example",
]

[storage]
blossom = [
  "https://blossom.example",
]

[identity]
pubkey = "npub1..."

[trust]
publishers = ["npub1...", "npub1..."]

[install]
user = false
```

When `identity.pubkey` is configured, npack reads that user's NIP-65
`kind:10002` relay list and adds read-capable relays. The identity is also
used to discover publisher Blossom servers through `kind:10063` events.

Publisher keys may be written as `npub` values. Internally, npack compares
canonical hexadecimal public keys so equivalent representations cannot create
separate identities.

### Managing trusted publishers: `npack trust`

You do not have to edit `config.toml` by hand:

```bash
npack trust add alice@example.com      # or an npub, or a 64-character hex key
npack trust list [--names]
npack trust remove npub1...
```

`add` shows the resolved key and asks before trusting it. A NIP-05 name such
as `alice@example.com` only helps you find the key: the name is looked up
through the domain, the resolved `npub` is displayed for you to check, and the
`npub` is what is stored. For an `npub` or hex key, `add` also looks up the
publisher's profile and labels any name it finds as verified (the domain
confirms the key) or unverified (the profile merely claims it). It will not
change the list without a confirmation: answer `y` at the prompt, or pass
`--yes`. When stdin is not a terminal, `--yes` is required, so a script cannot
trust someone by accident.

`list` is offline by default and prints `npub`s; `--names` looks up each
publisher's verified name (contacting relays and their domains). `remove`
accepts a name, `npub` or hex key and fails if that publisher was not trusted.
Only the `[trust].publishers` list is rewritten: other settings are kept, but
comments in `config.toml` are not, and a config file that does not parse is
left untouched.

A running `npackd` notices changes to `config.toml` on its next request, so
`npack trust` and the GUI's Settings tab stay in step without a restart.

## Trust and security

Trust publishers explicitly when possible:

```bash
npack search myapp \
  --trusted-publisher npub1... \
  --relay wss://relay.example
```

The v1 security model uses:

- A publisher-signed Nostr release event (`kind:9900`).
- A publisher-signed NIP-94 artifact event (`kind:1063`).
- SHA-256 verification of every downloaded archive.
- Signed revocation events (`kind:9901`).
- Explicit dependency, conflict and runtime capability declarations.

Do not use a personal primary Nostr key in automated publishing. Use a
dedicated package-publisher key and protect it carefully. Delegated release
keys are not part of protocol v1.

Post-install actions are deliberately limited and capability-gated. A
service file may be installed, but services are not automatically enabled or
started. Grant capabilities explicitly with:

```bash
npack install ./myapp.npk \
  --allow-capability service-manager
```

Review package metadata and publisher identity before granting capabilities.

## Publishing a release

### Registering a publisher key

Generate a new publisher key and store its `nsec` automatically in the
operating system credential store:

```bash
npack generate-key
```

The public `npub` is displayed; the private key is not printed by default.
Use `--show-secret` only when you are prepared to copy and back up the `nsec`
securely:

```bash
npack generate-key --show-secret
```

Register a dedicated publisher key once in the operating system credential
store. With no argument, npack prompts without echoing the key:

```bash
npack register
```

The key is stored through the platform credential APIs: Secret Service on
Linux, Keychain on macOS, or Credential Manager on Windows. It is not written
to `config.toml`. The `--stdin` form avoids exposing the nsec in shell history
or the process list. A positional form is also available, but is less safe:

```bash
npack register nsec1...
```

After registration, `release-event`, `publish`, and `revoke-event` can omit
`--secret-key`. Supplying `--secret-key` explicitly always overrides the
registered key. Registering another key replaces the previous `npack`
publisher credential for the current user.

Prepare an external manifest whose `artifact` and `sha256` identify the final
`.npk`, then create and verify the release metadata:

```bash
npack release-event ./myapp.manifest.json \
  --secret-key <publisher-secret-key> \
  > release.json

npack verify ./myapp.manifest.json
npack verify-event release.json ./myapp.manifest.json
```

Publish the artifact and both Nostr events:

```bash
npack publish ./myapp.manifest.json \
  --relay wss://relay.example \
  --server https://blossom.example
```

`npack publish` uploads the `.npk`, creates the NIP-94 artifact event, creates
the package release event, and sends both events to the configured relays.

A `.npk` already hosted somewhere else -- a GitHub raw file or release
asset, an S3 object, a plain web server -- does not need a Blossom
upload at all: pass its URL with `--artifact-url` instead of (or
together with) `--server`:

```bash
npack publish ./myapp.manifest.json \
  --relay wss://relay.example \
  --artifact-url https://github.com/<owner>/<repo>/releases/download/v1.0.0/myapp-1.0.0.npk
```

Repeat `--artifact-url` to list more than one, and combine it with
`--server` to also upload to Blossom; every location becomes a `url` tag
on the artifact event, and a client tries each in turn until the
downloaded bytes match the signed SHA-256 (`npack` has always verified
the hash this way -- artifact URLs were only ever transport, so the
protocol does not change). At least one of `--server`/`--artifact-url`
is required.

The store icon is uploaded to a Blossom server too unless it is already hosted:
`--icon-url <https-url>` skips that upload and signs the URL together with the
SHA-256 of the icon inside the package. The URL must serve the file directly,
because clients never follow a redirect when fetching an icon (a GitHub release
asset URL redirects; `https://raw.githubusercontent.com/<owner>/<repo>/<tag>/<path>`
does not). A file at the URL that differs from the packaged icon is rejected by
clients.

To revoke a published release:

```bash
npack revoke-event release.json \
  --secret-key <publisher-secret-key> \
  --reason "security issue"
```

Deletion is not treated as revocation because relays and clients may retain
copies of the original event.

## GitHub Actions

The repository contains a reusable reference workflow at
`.github/workflows/release.yml`. On a `v*` tag it builds a deterministic
Linux `.npk`, embeds package metadata, calculates the final hash, generates
an SPDX SBOM, creates a GitHub provenance attestation, uploads a GitHub
Release, and publishes to Nostr.

Configure:

- Repository variable `NOSTR_PUBLISHER` — the publisher public key.
- Repository variables `NOSTR_RELAYS` and `NOSTR_BLOSSOM_SERVERS` — one URL
  per line.
- Secret `NOSTR_SECRET_KEY` in the protected `release` environment — the
  dedicated publisher private key.

GitHub Actions is only a builder and publisher implementation. Forgejo,
GitLab, local builders and reproducible-build systems can publish the same
`.npk` and Nostr event format.

## Package maintenance

List installed packages:

```bash
npack list --user
```

Verify installed files and hashes:

```bash
npack verify-installed --user
```

Update a remote package:

```bash
npack update npub1.../myapp --user
```

Remove a package:

```bash
npack remove npub1.../myapp --user
```

Removal is refused when another installed package depends on the package or
when removing it would leave a required runtime capability unavailable.

### History and rollback

Every install, upgrade, downgrade, rollback and removal is recorded in
`history.jsonl` in the store. View it, and switch a package back to the version
it replaced:

```bash
npack history [<publisher>/<name>] [--limit 20] [--user]
npack rollback <publisher>/<name> [--user] [--allow-capability <capability>]
```

`rollback` works offline. npack keeps each installed version's archive and the
signed release metadata it cached when it resolved that version, so a rollback
is an ordinary transactional install of the stored archive, not a separate
restore mechanism. Before changing anything it checks that:

- history names an earlier version (a fresh install has none);
- the stored archive still hashes to the SHA-256 recorded for it;
- the signed release event cached for that version is still present and valid;
- no installed package depends on a version range the older version fails to
  satisfy.

If any check fails it says why and leaves the install untouched. Running
`rollback` again switches forward to the version you just left. It does not
check whether the older version has since been revoked (that needs the network):
run `npack update --check`, which reports revoked installs.

Old versions' archives are kept until the package is removed, so they use disk
space; `remove` deletes all of them.

### Transaction locking and interrupted-install recovery

Every `install`/`remove` against a given store (system, `--user`, or a
`--store <path>` used for isolated/test installs) is serialized by an
exclusive lock, so running two npack operations against the same store at
once fails fast rather than racing:

```text
Error: another npack operation is already in progress (pid 12345)
```

If an npack process is killed mid-install or mid-remove (a crash, `kill -9`,
a power loss), the next operation against that store detects that the pid
holding the lock is no longer running, prints a recovery notice, restores
`installed.json` to its state before the interrupted transaction, and
removes any package directory that transaction had newly created -- then
proceeds normally. No manual cleanup of the store is needed after an
interrupted operation.

## Troubleshooting

### Start with `npack doctor`

```bash
npack doctor            # a report, grouped by area, with a suggested fix for each finding
npack doctor --offline  # skip the relay and download-server checks
npack doctor --json     # the same findings as JSON, for scripts and other tools
```

`doctor` changes nothing. It checks the config file (and that every trusted
publisher entry is a valid key), the package store (writable, a stale lock or
transaction left by an interrupted install, a readable `installed.json`, and
that every installed package's archive still matches its recorded hash and its
files are still there), the catalogue (present and how old), whether `npackd` is
answering, whether helpers such as `notify-send` and PolicyKit are installed and
the daily update check is set up, and finally whether each configured relay and
download server answers (all at once, so a dead one costs at most eight
seconds).

Each finding is one of four kinds: a tick (fine), a dot (just worth knowing),
`!` (something is off but npack still works) or `✗` (npack can't do its job).
The command exits with status 1 only when there is a `✗`, so it can be used in
scripts. A missing relay is a warning; having *no* working relay is a failure.
A missing optional helper is never more than a note.

The GUI has the same check as a **Health** tab, driven by the daemon's
`RunDoctor` method. It runs once when the app starts, shows a badge on the tab
(`Health (2)` for warnings, `Health ✗` when something stops npack working), and
shows a warning strip on every tab for the latter. "Check again" reruns it. When
run by the daemon, the "is the service running" check is replaced by "This
service is running", since a service cannot meaningfully ask itself.

It runs even when the config file is broken, which stops every other command at
startup, and shows the line the parser objected to. Pass `--user`, `--system` or
`--store` to choose which package store to inspect, as with other commands. It
only reads: a stale lock is reported but left for the next install to recover,
and the one thing it writes is a tiny file it creates and deletes in the store to
test that the folder is writable.

### `no verified release found`

Add more package relays with `--relay`, use the publisher-qualified package
reference, or check that the publisher is not filtered out by a trust list.

### `no artifact mirror returned the expected SHA-256`

The storage server may be unavailable or may not contain the blob. Add a
known Blossom server with `--server`. A server returning different bytes is
rejected automatically.

### Permission denied during installation

Use `--user` for a user-local install, or run the default system install with
the privileges required by the host filesystem.

### Dependency or runtime capability failure

Read the package's declared dependencies and `runtime_requires`. Install the
missing package/provider first, or use a release built for the correct host
OS and architecture.

## Command summary

Publish a signed Nostr text note using the registered key and configured
write relays:

```bash
npack announce "npack is now available!"
```

Use `--secret-key` for a one-off key override, or `--relay` to select relays
explicitly. To attach a signed package release event, pass its JSON file with
`--release-event`. The command adds package name, version, platform, SHA-256,
a `nostr:nevent...` link, and NIP-27-compatible event tags automatically:

```bash
npack announce --release-event ./release.json "npack 0.2.6 is available!"
```

Generate the installed command reference:

```bash
npack man > npack.1
man ./npack.1
```

Release packages install the same page as `share/man/man1/npack.1`.

```text
npack pack <directory> --output <file.npk>
npack init <directory> --name <name> --publisher <npub-or-hex> [--version <semver>] [--os <os>] [--arch <arch>]
npack manifest <file.npk> --output <manifest.json>
npack install <file.npk> [--user|--system|--store <path>]
npack verify <file.npk-or-manifest.json>
npack refresh [--relay <url>] [--pubkey <hex>] [--store <path>]
npack search <query> [--refresh] [--no-cache] [--relay <url>] [--category <name> | --uncategorised] [--sort name|recent] [--limit <n>] [--store <path>]
npack categories [--refresh] [--all-platforms] [--relay <url>] [--store <path>]
npack info [<publisher>/]<name> [--trusted-publisher <hex>] [--store <path>]
npack trust add <name@domain|npub|hex> [--yes] [--relay <url>]
npack trust remove <name@domain|npub|hex>
npack trust list [--names] [--relay <url>]
npack install <publisher>/<name> [options]
npack install-ref <publisher>/<name> [options]  # compatibility alias
npack resolve <publisher>/<name> --relay <url> [options]
npack update <publisher>/<name> [options]
npack list [--user|--system]
npack verify-installed [--user|--system]
npack doctor [--user|--system] [--store <path>] [--offline] [--json]
npack remove <publisher>/<name> [--user|--system]
npack history [<publisher>/<name>] [--limit <n>] [--user|--system]
npack rollback <publisher>/<name> [--user|--system] [--allow-capability <capability>]
npack publish <manifest> --secret-key <key> [options]
npack appstream <file.npk> [--output <metainfo.xml>]
npack daemon [--socket <path>] [--system]
```
