# EVE Killmail Publisher

EVE Killmail Publisher (`ekmp`) is a Rust desktop utility for reviewing EVE Online character killmails
and explicitly submitting selected killmails to zKillboard.

## Current Features

- EVE SSO PKCE authentication with the `esi-killmails.read_killmails.v1` scope
- Multiple authenticated characters
- Removal of authenticated characters, their stored refresh tokens, and their
  unshared cached killmails
- Recent killmail retrieval from ESI
- Cached zKillboard reporting status for each eligible killmail, refreshed for unknown entries at startup
- Session-only results for explicitly submitted killmails, with zKillboard links
- Configurable protected victim characters and corporations excluded from bulk posting
- A posting summary showing bulk-eligible, protected, and still-unchecked killmails
- A separate `Post to zKillboard` button for each loaded killmail
- Confirmed bulk submission of all unreported killmails

Killmails are never submitted automatically.

Authenticated characters and their corporations are automatically protected.
Additional victim character and corporation IDs can be added under `Protected
victims`. Killmails involving protected victims are excluded from bulk
submission but can still be submitted individually with the explicit `Post
anyway` button. Protected killmails are hidden from the recent-killmail list by
default and can be displayed with the persisted `Show protected killmails`
checkbox.

## Setup

Before building a release, create an application at the
[EVE Developer Portal](https://developers.eveonline.com/), register
`http://127.0.0.1:17842/callback` as its callback URL, and replace
`REPLACE_WITH_YOUR_EVE_CLIENT_ID` in `src/auth.rs` with its client ID. EVE
defines client IDs as public identifiers, so it does not need to be kept out of
the repository or compiled application. Never add the client secret.

Users can then run the application, authenticate one or more characters, load
recent killmails, and use an individual post button when desired. They do not
need their own EVE developer application or credentials.

The local configuration is stored in `~/.config/ekmp/ekmp.json` on Linux and
`%APPDATA%\ekmp\ekmp.json` on Windows. It contains application preferences, a
local snapshot of the most recently loaded killmails, and their zKillboard
status cache. The snapshot is displayed immediately on the next startup. Full
killmail records already reported to zKillboard are removed from the snapshot;
only their compact status-cache entries are retained. Unreported results are
checked again after 15 minutes.

OAuth refresh tokens are stored in the operating system credential store:
Secret Service on Linux (GNOME Keyring or KDE/KWallet), Keychain on macOS, and
Credential Manager on Windows. Linux requires an unlocked Secret Service
provider; macOS and Windows provide their credential stores as part of the
operating system. If the credential store is unavailable or fails on any
platform, the application falls back to storing the affected refresh token in
`ekmp.json` and displays a persistent security warning. Treat that file as
sensitive whenever the warning is present. PKCE means a client secret is never
needed or stored by this desktop application.

## Development

```sh
cargo run
cargo test
```

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

### KDE/Wayland development launcher

Wayland desktops resolve taskbar and launcher icons through a desktop-entry
file, rather than the running application's window icon. To create a
development launcher that uses the debug build and the repository icon, run
this once from the repository:

```sh
scripts/setup-dev-launcher.sh
```

Then build normally with `cargo build` and launch **EVE Killmail Publisher
(development)** from the application menu. Cargo always replaces
`target/debug/ekmp` in place, so
the launcher does not need to be reinstalled after recompiling. Re-run the
script only if the repository moves to a different directory.
