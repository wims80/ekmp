# akmp

akmp is a Rust desktop utility for reviewing EVE Online character killmails
and explicitly submitting selected killmails to zKillboard.

## Current Features

- EVE SSO PKCE authentication with the `esi-killmails.read_killmails.v1` scope
- Multiple authenticated characters
- Recent killmail retrieval from ESI
- Cached zKillboard reporting status for each eligible killmail
- A separate `Post to zKillboard` button for each loaded killmail
- Confirmed bulk submission of all unreported killmails

Killmails are never submitted automatically.

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
cache. The snapshot is displayed immediately on the next startup. Reported
results are retained, while unreported results are checked again after 15
minutes.

## Development

```sh
cargo run
cargo test
```
