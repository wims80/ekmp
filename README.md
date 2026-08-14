# akmp

akmp is a Rust desktop utility for reviewing EVE Online character killmails
and explicitly submitting selected killmails to zKillboard.

## Current Features

- EVE SSO authentication with the `esi-killmails.read_killmails.v1` scope
- Multiple authenticated characters
- Recent killmail retrieval from ESI
- A separate `Post to zKillboard` button for each loaded killmail

Killmails are never submitted automatically.

## Setup

1. Create an application at the [EVE Developer Portal](https://developers.eveonline.com/).
2. Register `http://127.0.0.1:17842/callback` as the callback URL.
3. Run the application with `cargo run`.
4. Enter the application client ID and secret, then authenticate one or more characters.
5. Load recent killmails and use an individual post button when desired.

The local configuration is stored in `~/.config/akmp/akmp.json`. It contains
OAuth refresh tokens and should be treated as sensitive. The current prototype
does not encrypt this file.

## Development

```sh
cargo run
cargo test
```
