# EVE Killmail Publisher

EVE Killmail Publisher (`ekmp`) reviews EVE Online killmails and explicitly submits selected killmails to zKillboard. It is a Linux command-line tool with an optional refresh service. Release builds also include an optional desktop interface; source builds include it only with the `gui` feature.

Killmails are never submitted automatically. Authenticated characters and their corporations are protected victims automatically. Manually protected victims are hidden by default, excluded from bulk posting, and can only be sent through an explicit individual `Post anyway` action.

## Command line

Running `ekmp` with no arguments displays help. `--json` produces structured results on stdout; prompts, progress, and diagnostics use stderr.

```sh
ekmp characters add
ekmp refresh
ekmp list
ekmp show KILLMAIL_ID
ekmp post KILLMAIL_ID
```

Use `ekmp gui` to open the desktop interface in a build that includes it. The remaining commands are:

| Command | Purpose |
| --- | --- |
| `characters list` | List authenticated characters. |
| `characters add [--no-browser]` | Authenticate with same-machine PKCE. `--no-browser` prints the authorization URL. |
| `characters remove ID [--yes]` | Remove a character, its credentials, and unshared cached killmails after confirmation. |
| `refresh` | Refresh recent killmails and reporting statuses. |
| `list` / `show ID` | Read cached killmails without network requests. |
| `post ID [--post-anyway] [--yes]` | Explicitly post one confirmed-unreported killmail. Protected victims require `--post-anyway`. |
| `post --all [--yes]` | Post only confirmed, still eligible killmails. Protected victims are never included. |
| `protect list/add/remove` | Manage character, corporation, and individual-killmail protection. |
| `config get/set` | Manage `refresh-interval` and `show-protected-killmails`. |
| `service run [--interval 15m]` | Run the optional foreground refresh service. |
| `status` | Display cached counts, refresh timing, service state, and API cooldowns. |

Lists use the saved protected-visibility preference. `--show-protected` and `--hide-protected` override it for one invocation. `post` verifies reporting status before confirmation. Noninteractive post and removal operations require `--yes`; they fail rather than prompting when no terminal is available.

The refresh service performs refreshes only: it never posts, opens a browser, or initiates authentication. Its default interval is 15 minutes after a cycle finishes; `--interval` overrides that process only. Concurrent foreground work returns busy instead of waiting silently.

## Installation

Download the Linux archive from [GitHub Releases](https://github.com/wims80/ekmp/releases). Releases support x86-64 Linux with glibc 2.35 or newer.

Extract `ekmp-*-x86_64-unknown-linux-gnu.tar.gz`, enter it, and run:

```sh
./install.sh
```

This installs `ekmp` in `~/.local/bin`, a desktop launcher, and its icon for the current user. The desktop launcher runs `ekmp gui`; invoke `ekmp` directly for the CLI. Remove the program, launcher, and icon with `./install.sh --uninstall`; settings and caches remain in place.

The archive contains an opt-in `ekmp-refresh.service` template. To enable the refresh service for the authenticated user:

```sh
mkdir -p ~/.config/systemd/user
cp ekmp-refresh.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now ekmp-refresh.service
```

It expects `ekmp` in `~/.local/bin`. Installation never enables the service. Remove it with `systemctl --user disable --now ekmp-refresh.service` and `rm ~/.config/systemd/user/ekmp-refresh.service`.

## Authentication, data, and caches

The release contains the public EVE client ID and loopback callback registration. Users must never create, enter, request, or share a client secret. PKCE refresh tokens use the Secret Service credential store when possible. When it is unavailable, as on many headless hosts, tokens fall back to `credentials.json`, which is readable only by the user and removed once no fallback tokens remain.

Files follow the XDG Base Directory Specification:

| File | Contents |
| --- | --- |
| `$XDG_CONFIG_HOME/ekmp/config.toml` (`~/.config/ekmp/`) | Preferences: `refresh-interval-secs` and `show-protected-killmails`. Edit it by hand or with `ekmp config set`; unknown keys are rejected. |
| `$XDG_STATE_HOME/ekmp/state.json` (`~/.local/state/ekmp/`) | Characters, cached unreported killmails, protection, compact reported-ID information, scheduling, and API cooldowns. |
| `$XDG_STATE_HOME/ekmp/credentials.json` | Fallback refresh tokens; present only when the credential store failed. |
| `$XDG_CACHE_HOME/ekmp/` (`~/.cache/ekmp/`) | ESI responses (SQLite) and, in GUI builds, portraits and logos. Safe to delete. |

Full reported killmail records and session-only successful submission results are not persisted.

Do not attach `credentials.json`, `state.json`, refresh tokens, authorization URLs, or killmail hashes to public issue reports.

## Development

The default build is the CLI and refresh service. It omits eframe and image decoding and works without a display:

```sh
cargo build
./target/debug/ekmp --help
```

The desktop interface is opt-in:

```sh
cargo run --features gui -- gui
cargo test --all-features
```

### Offline simulation

The `dev-tools` feature uses compiled-in synthetic data and cannot contact EVE, zKillboard, the image service, or a credential store. Global development flags precede the command:

```sh
cargo run --features dev-tools -- --scenario mixed gui
cargo run --features dev-tools -- --scenario errors list
cargo run --features dev-tools -- --scenario mixed --dev-state target/ekmp-dev-state.json list
```

The simulator preserves the same explicit submission policy as live operation. Scenario fixtures are in `dev/scenarios/`; they must contain invented IDs, hashes, names, and outcomes. See [SIMULATOR-RUNBOOK.md](SIMULATOR-RUNBOOK.md).

`EGUI_INSPECTION=1` is available only in a dev-tools GUI build and only with a simulation scenario; it is rejected for live runs.

## EVE notice

© 2026 Fenris Creations. All rights reserved. EVE Online® and Fenris Creations™ and all related logos and other elements are trademarks of Fenris Creations. EVE Killmail Publisher is an independent, non-commercial third-party tool and is not affiliated with or endorsed by Fenris Creations.
