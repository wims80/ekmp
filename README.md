# akmp

akmp is a Rust desktop utility for reviewing EVE Online character killmails
and explicitly submitting selected killmails to zKillboard.

## Current Features

- EVE SSO PKCE authentication with the `esi-killmails.read_killmails.v1` scope
- Multiple authenticated characters
- Recent killmail retrieval from ESI
- Cached zKillboard reporting status for each eligible killmail, refreshed for unknown entries at startup
- Session-only results for explicitly submitted killmails, with zKillboard links
- Configurable protected victim characters and corporations excluded from bulk posting
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

The local configuration is stored in `~/.config/akmp/akmp.json`. It contains
OAuth refresh tokens and should be treated as sensitive. PKCE means the client
secret is not needed or stored by this desktop application. The current
prototype does not encrypt the configuration file. It also contains a local
snapshot of the most recently loaded killmails and their zKillboard status
cache. The snapshot is displayed immediately on the next startup. Full
killmail records already reported to zKillboard are removed from the snapshot;
only their compact status-cache entries are retained. Unreported results are
checked again after 15 minutes.

## Development

```sh
cargo run
cargo test
```

### KDE/Wayland development launcher

Wayland desktops resolve taskbar and launcher icons through a desktop-entry
file, rather than the running application's window icon. To create a
development launcher that uses the debug build and the repository icon, run
this once from the repository:

```sh
scripts/setup-dev-launcher.sh
```

Then build normally with `cargo build` and launch **akmp (development)** from
the application menu. Cargo always replaces `target/debug/akmp` in place, so
the launcher does not need to be reinstalled after recompiling. Re-run the
script only if the repository moves to a different directory.
