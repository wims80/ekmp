# EVE Killmail Publisher

EVE Killmail Publisher (`ekmp`) reviews EVE Online killmails and explicitly submits selected killmails to zKillboard. It is a Linux command-line tool with an optional refresh service. Release builds also include an optional desktop interface; source builds include it only with the `gui` feature.

Killmails are never submitted automatically. Authenticated characters and their corporations are protected victims automatically. Manually protected victims are hidden by default, excluded from bulk posting, and can only be sent through an explicit individual `Post anyway` action.

## Command line

Running `ekmp` with no arguments displays help. Results are readable tables and summaries on stdout, with times in UTC (EVE time). For scripts, `--json` prints the same results as JSON, a stable interface whose fields do not change with the text layout. Prompts, progress, and diagnostics always use stderr.

Exit codes: `0` success, `1` error or partial failure, `2` invalid usage, `3` another ekmp operation is in progress, `130` cancelled. A stopped `service run` exits `0`.

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
| `characters add [--no-browser \| --paste]` | Authenticate with EVE SSO (PKCE). `--no-browser` prints the authorization URL; `--paste` signs in from another machine (see below). |
| `characters remove ID [--yes]` | Remove a character, its credentials, and unshared cached killmails after confirmation. |
| `refresh` | Refresh recent killmails and reporting statuses. |
| `list` / `show ID` | Read cached killmails without network requests. |
| `post ID [--post-anyway] [--yes]` | Explicitly post one confirmed-unreported killmail. Protected victims require `--post-anyway`. |
| `post --all [--yes]` | Post only confirmed, still eligible killmails. Protected victims are never included. |
| `protect list/add/remove` | Manage character, corporation, and individual-killmail protection. |
| `config get/set` | Manage `refresh-interval` (at least 5 minutes) and `show-protected-killmails`. |
| `service run [--interval 15m]` | Run the optional foreground refresh service. The interval must be at least 5 minutes. |
| `status` | Display cached counts, refresh timing, service state, and API cooldowns. |

Lists use the saved protected-visibility preference. `--show-protected` and `--hide-protected` override it for one invocation. `post` verifies reporting status before confirmation. Noninteractive post and removal operations require `--yes`; they fail rather than prompting when no terminal is available.

The refresh service performs refreshes only: it never posts, opens a browser, or initiates authentication. Its default interval is 15 minutes after a cycle finishes; `--interval` overrides that process only. Concurrent foreground work returns busy instead of waiting silently.

## Installation

### Arch Linux

`packaging/arch/PKGBUILD` builds the latest tagged release from source and installs it system-wide with pacman:

```sh
git clone https://github.com/wims80/ekmp.git
cd ekmp/packaging/arch
makepkg -si
```

It installs `ekmp` in `/usr/bin`, the `ekmp.service` systemd user unit in `/usr/lib/systemd/user/`, shell completions, man pages, and the desktop launcher. To upgrade, `git pull` and run `makepkg -si` again; remove it with `pacman -R ekmp`. Before switching from the release archive, run its `./install.sh --uninstall`: files under `~/.local` and `~/.config/systemd/user` take precedence over the package's.

### Other distributions

Download the Linux archive from [GitHub Releases](https://github.com/wims80/ekmp/releases). Releases support x86-64 Linux with glibc 2.35 or newer.

Extract `ekmp-*-x86_64-unknown-linux-gnu.tar.gz`, enter it, and run:

```sh
./install.sh
```

It installs for the current user, without administrator access:

| Installed file | Location |
| --- | --- |
| `ekmp` | `~/.local/bin/` |
| `ekmp.service` (systemd user unit; not enabled) | `~/.config/systemd/user/` |
| Shell completions for bash, zsh, and fish | `~/.local/share/bash-completion/completions/`, `~/.local/share/zsh/site-functions/`, `~/.local/share/fish/vendor_completions.d/` |
| Man pages (`man ekmp`, `man ekmp-post`, …) | `~/.local/share/man/man1/` |
| Desktop launcher and icon for `ekmp gui` | `~/.local/share/applications/`, `~/.local/share/icons/` |

bash and fish load the completions automatically. For zsh, add `fpath+=(~/.local/share/zsh/site-functions)` before `compinit` in `~/.zshrc`. Running `./install.sh` again upgrades in place; restart a running service afterwards with `systemctl --user restart ekmp`.

`./install.sh --uninstall` stops and disables the service and removes every installed file. Settings, state, and caches remain in place.

### Refresh service

Both installation methods install an opt-in systemd user unit, `ekmp.service`. It runs `ekmp service run` as the user who authenticated the characters, so it uses the same credentials and state. Installation never enables it. To enable it:

```sh
systemctl --user enable --now ekmp
```

Without the installer, copy the archive's `ekmp.service` to `~/.config/systemd/user/` and run `systemctl --user daemon-reload` first; the unit expects `ekmp` in `~/.local/bin`.

- Logs: `journalctl --user -u ekmp`. Each refresh logs one summary line; tokens, killmail hashes, and authorization URLs are never logged.
- Status: `ekmp status` reports whether the service is running.
- Without a login session, for example on a server: `loginctl enable-linger "$USER"` keeps user services running.
- Disable it with `systemctl --user disable --now ekmp.service`. A manually copied unit is removed with `rm ~/.config/systemd/user/ekmp.service`.

The service checks at least once a minute whether a refresh is due, so `config set` changes and new characters take effect without a restart. With no authenticated characters it waits rather than exiting. `systemctl --user stop` (SIGTERM) or Ctrl+C finishes the current request, saves, and exits with status 0.

## Signing in on a headless host

EVE SSO redirects the browser to `http://127.0.0.1:17842/callback` on the machine running the browser. Without `DISPLAY` or `WAYLAND_DISPLAY`, `ekmp characters add` prints the authorization URL instead of opening a browser. To sign in to a server from another machine, either:

- run `ekmp characters add --paste`, open the printed URL on any machine, sign in, and paste the address the browser is sent to (the page itself fails to load, which is expected); or
- forward the callback with `ssh -L 17842:127.0.0.1:17842 SERVER`, run `ekmp characters add` in that session, and open the printed URL in your local browser.

Either way ekmp checks the OAuth `state` and completes PKCE itself; the pasted address is single-use and cannot be redeemed without the PKCE verifier that only this ekmp process holds.

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

The `dev-tools` feature uses compiled-in synthetic data and cannot contact EVE, zKillboard, the image service, or a credential store. It is for debug builds only: `--scenario` and `--dev-state` do not exist in other builds, and combining `dev-tools` with `--release` fails to compile, so release binaries can never contain the simulator. The development flags precede the command; the GUI scenario also needs the `gui` feature:

```sh
cargo run --features dev-tools,gui -- --scenario mixed gui
cargo run --features dev-tools -- --scenario errors list
cargo run --features dev-tools -- --scenario mixed --dev-state target/ekmp-dev-state.json list
```

The simulator preserves the same explicit submission policy as live operation. Scenario fixtures are in `dev/scenarios/`; they must contain invented IDs, hashes, names, and outcomes. See [SIMULATOR-RUNBOOK.md](SIMULATOR-RUNBOOK.md).

`EGUI_INSPECTION=1` is available only in a dev-tools GUI build and only with a simulation scenario; it is rejected for live runs.

## API use

ekmp follows the [ESI best practices](https://developers.eveonline.com/docs/services/esi/best-practices/) and the [zKillboard API rules](https://github.com/zKillboard/zKillboard/wiki/API-(Killmails)):

- Every request identifies ekmp, its version, and this repository in `User-Agent`.
- ESI requests use `https://esi.evetech.net` with a pinned `X-Compatibility-Date`, never `/latest`. Deprecation warnings (`Warning: 299`) are reported once per route.
- ESI responses are cached until `Expires` and revalidated with `ETag`. Rate-limit (`X-Ratelimit-*`, 429) and error-limit (420) cooldowns are persisted, so they also hold across restarts and between the CLI and the service.
- zKillboard requests use gzip, are spaced at least one second apart across processes, and lookups are cached. Submissions are never retried automatically.
- Refreshes run at most every 5 minutes, and only one refresh runs at a time.

## EVE notice

© 2026 Fenris Creations. All rights reserved. EVE Online® and Fenris Creations™ and all related logos and other elements are trademarks of Fenris Creations. EVE Killmail Publisher is an independent, non-commercial third-party tool and is not affiliated with or endorsed by Fenris Creations.
