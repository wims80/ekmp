# EVE Killmail Publisher

EVE Killmail Publisher (`ekmp`) is a Rust desktop utility for reviewing EVE Online character killmails
and explicitly submitting selected killmails to zKillboard.

## Current Features

- EVE SSO PKCE authentication with the `esi-killmails.read_killmails.v1` scope
- Multiple authenticated characters
- Locally cached EVE portraits, organization logos, ship renders, and item icons
- Removal of authenticated characters, their stored refresh tokens, and their
  unshared cached killmails
- Automatic recent killmail refresh from ESI at application startup, with
  manual refresh available afterward
- Cached zKillboard reporting status for each eligible killmail, refreshed from
  the relevant paginated zKillboard feed for unknown entries at startup
- Session-only results for explicitly submitted killmails, with zKillboard links
- Configurable protected victim characters and corporations excluded from bulk posting
- Persisted protection flags for individual killmails
- A posting summary showing bulk-eligible, protected, and still-unchecked killmails
- A separate `Post to zKillboard` button for each killmail still confirmed as
  unreported, revealed by expanding its compact killmail card
- Confirmed bulk submission of all unreported killmails
- A review-focused dashboard with connected characters and protection controls
  beside a status summary and compact killmail cards with estimated ISK values
- Rich expanded killmail cards with victim and location context, independently
  scrollable aggressor and fitting/content panes, final-blow and top-damage
  identification, item quantities, and dropped/destroyed outcomes
- An expanded activity log that identifies characters, killmail IDs, source
  counts, and zKillboard status-check outcomes

Killmails are never submitted automatically.

Authenticated characters and their corporations are automatically protected.
Additional victim characters and corporations can be added by exact EVE name
or numeric ID under `Protected victims`. An individual killmail can also be
flagged or unflagged from its expanded card. Protection flags persist across
restarts and refreshes until removed or until the killmail is reported.
Protected killmails are excluded from bulk submission but, while confirmed as
unreported, can still be submitted individually with the explicit `Post anyway`
button. They are hidden from the recent-killmail list by default and can be
displayed with the persisted `Show protected killmails` checkbox.

## Installation

Download the archive for your operating system from the
[GitHub Releases](https://github.com/wims80/ekmp/releases) page. The initial
release supports x86-64 Linux systems with glibc 2.35 or newer and x86-64
Windows 10 or newer. macOS and Linux ARM64 are not supported.

### Linux

Extract `ekmp-*-x86_64-unknown-linux-gnu.tar.gz`, enter the extracted
directory, and run:

```sh
./install.sh
```

This installs `ekmp` in `~/.local/bin`, a launcher in
`~/.local/share/applications`, and its icon in the matching `hicolor` icon
directory. The launcher works with GNOME, KDE, and other desktops that follow
the freedesktop desktop-entry standard. To remove the installed program while
keeping your settings and cached data, run `./install.sh --uninstall` from the
same release archive.

You can also run the extracted `ekmp` executable directly; installing it is
only needed for an application-menu and taskbar icon.

### Windows

Extract `ekmp-*-x86_64-pc-windows-msvc.zip` and run `ekmp.exe`. Windows may
show a SmartScreen warning because the executable is not code signed. Verify
the archive against the `SHA256SUMS` file attached to the same release before
running it.

### Authentication and local data

The release already contains the EVE client ID and the required loopback
callback registration. Users do not need to create an EVE developer
application and must never enter, request, or share a client secret.

After starting the application, authenticate one or more characters and use an
individual post button when desired. On later starts, recent killmails refresh
automatically after cached zKillboard statuses are checked.
An in-progress character connection can be cancelled from the application and
times out after five minutes if the browser authorization is abandoned.

The local configuration is stored in `~/.config/ekmp/ekmp.json` on Linux and
`%APPDATA%\ekmp\ekmp.json` on Windows. It contains application preferences, a
local snapshot of the most recently loaded killmails, individual killmail
protection flags, and the zKillboard status cache. The snapshot is displayed
immediately on the next startup. Full killmail records already reported to
zKillboard are removed from the snapshot; only their compact status-cache
entries are retained. Unreported results are checked again after 15 minutes.
Matching unreported killmail snapshots are reused during refresh so their full
ESI detail is not downloaded repeatedly.

Character portraits, corporation and alliance logos, ship renders, and item
icons from the EVE image service are loaded when needed and cached separately in
`~/.cache/ekmp/images` on Linux (or `$XDG_CACHE_HOME/ekmp/images` when set) and
`%LOCALAPPDATA%\ekmp\images` on Windows. Cached images are refreshed after seven
days, with stale images retained as an offline fallback.

Cacheable ESI GET responses are stored separately in a bounded SQLite cache:
`~/.cache/ekmp/esi-cache.sqlite3` on Linux (or
`$XDG_CACHE_HOME/ekmp/esi-cache.sqlite3`) and
`%LOCALAPPDATA%\ekmp\esi-cache.sqlite3` on Windows. The cache follows ESI's
`Expires` and `ETag` headers, revalidating expired data conditionally. Full
killmail-detail responses are deliberately excluded so reported killmail
records are never retained there. Entity/type names are deduplicated and
resolved in bulk during refresh; system, constellation, region, and market-price
lookups use the HTTP cache. Displayed ISK totals use ESI average prices with
adjusted prices as a fallback and are estimates rather than live market quotes.

Configuration updates are written through a temporary file before replacing
the previous file. On Unix, the file is restricted to the current user. If the
existing configuration cannot be read or parsed, the application displays an
error and disables saving for that session rather than overwriting it.

OAuth refresh tokens are stored in the operating system credential store:
Secret Service on Linux (GNOME Keyring or KDE/KWallet), Keychain on macOS, and
Credential Manager on Windows. Linux requires an unlocked Secret Service
provider; macOS and Windows provide their credential stores as part of the
operating system. If the credential store is unavailable or fails on any
platform, the application falls back to storing the affected refresh token in
`ekmp.json` and displays a persistent security warning. Treat that file as
sensitive whenever the warning is present. PKCE means a client secret is never
needed or stored by this desktop application.

Do not attach `ekmp.json`, refresh tokens, authorization URLs, or killmail
hashes to public issue reports. Use GitHub's private vulnerability reporting
for security-sensitive reports.

## Support and contributions

Report reproducible bugs through [GitHub Issues](https://github.com/wims80/ekmp/issues).
Pull requests are welcome for review, but the repository has a single
maintainer and does not grant write access to outside contributors.

## EVE notice

© 2026 Fenris Creations. All rights reserved. EVE Online® and Fenris
Creations™ and all related logos and other elements are trademarks of Fenris
Creations. EVE Killmail Publisher is an independent, non-commercial
third-party tool and is not affiliated with or endorsed by Fenris Creations.

## Development

```sh
cargo run
cargo test
```

### Offline simulation

Development builds can run against synthetic data without authenticating with EVE or contacting
ESI, the EVE image service, the system credential store, or zKillboard:

```sh
cargo run --features dev-tools -- --scenario mixed
cargo run --features dev-tools -- --scenario errors
```

The `mixed` scenario covers eligible, protected, already reported, and shared-source killmails, plus
a detailed fitting, nested cargo, player attackers, and an NPC attacker for expanded-card testing. The
`errors` scenario contains a confirmed-unreported killmail whose simulated submission fails. Each
run starts from its fixture and keeps changes in memory. To debug persistence separately, provide an
explicit development-only state file:

```sh
cargo run --features dev-tools -- --scenario mixed --dev-state target/ekmp-dev-state.json
```

Simulation is compile-time opt-in, displays a visible banner, uses zero request spacing, and cannot
fall through to the live integrations. Its submission results are recorded in memory and still
require the same explicit individual or confirmed bulk actions as production. Scenario definitions
live in `dev/scenarios/` and must contain only invented IDs, hashes, names, and outcomes.
See [SIMULATOR-RUNBOOK.md](SIMULATOR-RUNBOOK.md) for manual workflows, scenario authoring, and
agent-driven UI testing.

### Agent-driven UI testing

The `dev-tools` feature enables eframe's AccessKit inspection support. Install and configure the
`egui_mcp` MCP server for the development agent, then launch an inspectable simulator:

```sh
cargo install --locked egui_mcp
EGUI_INSPECTION=1 cargo run --features dev-tools -- --scenario mixed
```

An agent can inspect the semantic UI tree, activate uniquely labelled controls, enter protected
victim data, and capture screenshots without relying on screen coordinates. For safety, the
application refuses to start with `EGUI_INSPECTION` enabled unless a simulator scenario was also
selected. Repeatable headless UI workflows use `egui_kittest` and run as part of
`cargo test --all-features`.

The real HTTP adapters are tested against short-lived localhost `httpmock` servers. No standalone
mock service or database is required.

### Windows

Install Rust with the default `x86_64-pc-windows-msvc` toolchain and install
the Visual Studio Build Tools with the **Desktop development with C++**
workload, including a Windows SDK. The build automatically locates the Windows
SDK resource compiler when it is not already on `PATH`:

```powershell
cargo run
cargo build --release
```

The release executable is `target\release\ekmp.exe`. The build embeds
`assets/windows/app-icon.ico` as its executable icon; the running window uses
the embedded PNG icon. Windows settings and cached state are saved beneath
`%APPDATA%\ekmp`.

To verify Windows Credential Manager without using an EVE refresh token, run
the opt-in integration test from an interactive, signed-in PowerShell session.
It creates one temporary credential and deletes it before exiting:

```powershell
cargo test --all-features windows_credential_manager_round_trip -- --ignored
```
