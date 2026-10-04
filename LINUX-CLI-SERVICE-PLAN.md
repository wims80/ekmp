# Plan: Linux CLI and refresh service first

Goal: make `ekmp` a Linux command-line tool with a systemd user service. A
plain `cargo build` should produce that by default. The code should be simple
and standard, and it should follow the published ESI and zKillboard rules.

The product rules in `AGENTS.md` stay the same. Nothing is posted
automatically. The service only refreshes data. Bulk posting never includes a
protected victim.

## Where the code is today

| Area | Today | Problem |
| --- | --- | --- |
| Default build | `default = ["gui"]` pulls in eframe, image and egui_kittest | Linux CLI and server users build a GUI they don't use |
| Binary | One `ekmp` binary; `service run` is a foreground loop | Fine. Keep one binary. |
| Service loop | Polls `next_refresh_delay` every 1–2 s, and every poll locks and parses `ekmp.json` | Wasted work. The service should sleep until the next refresh is due. |
| Signals | `ctrlc = "3"` is built without the `termination` feature | `systemctl stop` sends SIGTERM, which ekmp doesn't handle, so the process is killed in the middle of an operation |
| Paths | `~/.config/ekmp/ekmp.json` built from a hard-coded `$HOME/.config`; `$XDG_CONFIG_HOME` is ignored | Not XDG compliant. Config, state and secrets share one file. |
| Secrets | Secret Service, with the token saved in `ekmp.json` if that fails | On a headless server or linger session, Secret Service usually isn't there, so the token ends up in the main state file |
| Auth | Loopback callback on `127.0.0.1:17842`, plus `--no-browser` | On a headless host this only works with an SSH tunnel, and that isn't documented |
| ESI base | `https://esi.evetech.net/latest` | CCP says not to use `/latest`. Current ESI expects `X-Compatibility-Date`. |
| Refresh interval | `parse_interval` accepts `1s` | A user could make the service hammer ESI and zKillboard |
| Human output | The default output is pretty-printed JSON | Not what people expect from a CLI. JSON belongs behind `--json`. |
| `src/cli.rs` | 693 lines: parsing, dispatch, output, service loop, prompts | Too much for one file |
| Platforms | Windows build, packaging, CI and `build.rs`; macOS TODOs | Extra upkeep for a Linux-first tool |

## API rules this plan follows

### ESI ([best practices](https://developers.eveonline.com/docs/services/esi/best-practices/), [rate limiting](https://developers.eveonline.com/docs/services/esi/rate-limiting/), [versioning](https://developers.eveonline.com/blog/esi-endpoint-versioning-important-info-and-best-practices))

1. **User-Agent.** Required. Include the app name and version (we do), plus a
   contact. CCP strongly prefers an email; a source URL is accepted. We send
   the source URL today. Deciding whether to add an email is in Phase 0.
2. **Compatibility date.** Use `https://esi.evetech.net` without a version
   label. Send `X-Compatibility-Date: YYYY-MM-DD` as one constant. Bump that
   date only on purpose, after re-testing. Never use `/latest`.
3. **Deprecation warnings.** If a response has a `Warning: 299` header, log it
   once per route so a deprecated route is noticed.
4. **Caching.** Don't request a resource again before its `Expires` time. Use
   `If-None-Match` with the `ETag`. A 304 costs fewer tokens. The SQLite ESI
   cache already does this. Keep it.
5. **Floating-window rate limit.** Budgets are tracked per group and per user
   through the `X-Ratelimit-*` headers. A 2xx costs 2 tokens, a 3xx costs 1,
   a 4xx costs 5, and a 5xx costs 0. On 429, wait for `Retry-After`. The
   current cooldown logic already handles this. Add tests for the new header
   names and keep requests sequential, never in parallel.
6. **Error limit.** On a 420, or when `X-ESI-Error-Limit-Remain` reaches 0,
   stop until `X-ESI-Error-Limit-Reset`. This is already done. Keep the
   cooldown persisted so a restarted service respects it.

### zKillboard ([API wiki](https://github.com/zKillboard/zKillboard/wiki/API-(Killmails)))

1. Send a descriptive `User-Agent` and `Accept-Encoding: gzip`. Both are done.
2. Every URL ends with `/`. This is done.
3. Don't hammer the server, and space requests out. Today requests are at
   least 1 s apart, and that spacing is stored on disk so it holds across
   processes. Keep it.
4. Cache lookups locally. A query is reused for up to 1 h or until its
   `Expires` time. This is done.
5. Respect `Retry-After` on errors. This is done.
6. Never retry a submission automatically. The single-shot client has no
   redirects and no retries. Keep it.

### What this plan adds

- **Minimum refresh interval of 5 minutes.** Enforce it in `parse_interval`,
  in `Core::set_refresh_interval`, and when an interval is loaded. The
  default stays at 15 minutes. Recent-killmail data doesn't change faster
  than this, so polling more often only uses up budget.
- **One instance.** Keep the service lock so two services, or a service and a
  manual refresh, can't run at the same time.
- **Restart backoff.** `Restart=on-failure` with `RestartSec=60`, and keep the
  persisted cooldowns. A crash loop then can't turn into a request storm.

## Target design

```
ekmp                      one binary, default features = []
├── CLI commands          characters, refresh, list, show, post, protect, config, status
└── service run           long-running refresh loop for systemd --user
                          refreshes only: never posts or authenticates, never opens a browser

Files (XDG):
  $XDG_CONFIG_HOME/ekmp/config.json        preferences (refresh interval, show protected)
  $XDG_STATE_HOME/ekmp/state.json          characters, cached killmails, status, schedule, cooldowns
  $XDG_STATE_HOME/ekmp/credentials.json    0600, used only if Secret Service fails
  $XDG_CACHE_HOME/ekmp/esi.sqlite          ESI response cache (can be deleted safely)

systemd:
  ~/.config/systemd/user/ekmp.service      Type=simple, logs go to journald
```

Why these choices:

- **One binary, `ekmp service run`.** This is the usual pattern (restic,
  syncthing, rclone). There's one thing to install, and the code is shared.
- **A systemd *user* service, not a system one.** The refresh tokens and state
  belong to the user who logged in. For servers, document
  `loginctl enable-linger $USER`. A system unit would need its own user
  account and its own credential store. That adds complexity and gains
  nothing.
- **Logging to stderr as plain lines without timestamps.** journald adds the
  timestamp and the unit name. `--json` produces JSON lines for scripts. No
  logging crate is needed.
- **No `sd_notify`.** `Type=simple` is enough here, and it avoids a new
  dependency.

## Phases

Each phase can be merged on its own and passes the four checks in `AGENTS.md`.

### Phase 0: Decisions (decided 2026-10-05)

1. **GUI:** stays as an opt-in `gui` feature. It is not built by default.
2. **Windows and macOS:** dropped from CI, packaging, `build.rs` and the docs.
   Keep the code portable where that costs nothing.
3. **User-Agent contact:** the project has no email. Keep the repository URL
   (`https://github.com/wims80/ekmp`) as the contact. ESI accepts a source
   URL, so the current `USER_AGENT` doesn't change.
4. **New dependencies:** approved. Add `clap_complete` and `clap_mangen` to
   generate shell completions and a man page.

### Phase 1: Default build is CLI and service

- In `Cargo.toml`, set `default = []` and keep `gui` as an opt-in feature.
  Enable `ctrlc` with `features = ["termination"]` so SIGTERM and SIGHUP
  cancel cleanly.
- `egui_kittest` stays a plain dev-dependency. Cargo doesn't support optional
  dev-dependencies, so the default `cargo test` still compiles egui, but
  `cargo build` doesn't.
- Delete `build.rs` and `assets/windows/`. Windows was the only thing they
  served.
- Release builds use `--features gui` so the release binary keeps the GUI.
  The `.desktop` launcher installs only when that binary is in the archive.
- CI runs on Ubuntu only, with two jobs. The default job runs `check`,
  `clippy` and `test` with no features and with `--features dev-tools`. The
  `gui` job runs with `--all-features`.
- Update `AGENTS.md` (Architecture section) and the README in the same change.

### Phase 2: XDG paths and separate files

- Add `persistence::paths` with `config_dir()`, `state_dir()` and
  `cache_dir()`. Each follows `$XDG_*_HOME` and falls back to `$HOME`. Create
  directories with mode 0700.
- Split `Store` into `Config` (preferences) and `State` (everything else). Each
  is saved atomically using the existing `persist_to_path`. Move the operation
  lock and the service lock next to `state.json`.
- Store the fallback refresh tokens in their own `credentials.json` (mode
  0600). `state.json` then never contains secrets.
- Migration: none. AGENTS.md says compatibility isn't required. Document one
  `characters add` per character after upgrading.

### Phase 3: API compliance

- `esi/mod.rs`: use `ESI = "https://esi.evetech.net"` and add a
  `COMPATIBILITY_DATE` constant. `client.rs` sends `X-Compatibility-Date` on
  every request. Update the httpmock fixtures and check that every route still
  answers, especially `/killmails/{id}/{hash}/` and `/characters/{id}/killmails/recent/`.
- `client.rs`: log `Warning: 299` once per route at warning level, through
  `CoreEvent`.
- `http.rs`: `USER_AGENT` stays as it is: name, version and repository URL
  (decision 0.3). Keep the test that checks the format.
- `cli.rs` and `core/refresh.rs`: add a `MIN_REFRESH_INTERVAL` of 300 s and
  enforce it everywhere an interval enters. Add tests for 299 s (rejected) and
  300 s (accepted).
- Tests: send `X-Compatibility-Date`; record a 429 cooldown with the new
  `X-Ratelimit-*` headers; log the 299 warning.

### Phase 4: Service

- Move the service loop into `src/cli/service.rs`. It sleeps until
  `min(next_due, 60 s)`. The 60 s cap picks up `config set` changes and new
  characters. `Cancellation::wait` replaces the 1–2 s polling.
- SIGTERM or SIGINT finishes the current request, persists, releases the
  locks, and exits 0. The service no longer exits 130, which systemd treats as
  a failure.
- If no characters are configured, log it once and wait without exiting. This
  avoids a restart loop.
- Log one line per cycle, e.g. `refresh ok: 3 new unreported, 1 protected,
  next at 14:35`. Log errors as `refresh deferred: …`. Never log tokens,
  hashes or authorization URLs.
- `packaging/linux/ekmp.service`:

  ```ini
  [Unit]
  Description=EVE Killmail Publisher refresh service (never posts killmails)
  Documentation=https://github.com/wims80/ekmp
  After=network-online.target
  Wants=network-online.target

  [Service]
  Type=simple
  ExecStart=%h/.local/bin/ekmp service run
  Restart=on-failure
  RestartSec=60
  NoNewPrivileges=yes
  LockPersonality=yes
  RestrictRealtime=yes
  SystemCallArchitectures=native

  [Install]
  WantedBy=default.target
  ```

  Only use hardening directives that work in user units.
  `ProtectSystem` and similar options need user namespaces and fail silently
  on some distributions, so leave them out.
- `ekmp status` already reports whether the service is running, using the
  service lock. Keep that. Document `journalctl --user -u ekmp` for logs.

### Phase 5: Authentication on headless hosts

- If neither `DISPLAY` nor `WAYLAND_DISPLAY` is set, behave as if
  `--no-browser` was passed.
- Add `characters add --paste`. ekmp prints the authorization URL and the user
  opens it on any machine. After login, the browser goes to
  `http://127.0.0.1:17842/callback?code=…&state=…`, which fails to load on
  that machine. The user pastes that URL back into the terminal. ekmp checks
  `state` and exchanges the code with the PKCE verifier. No tunnel or
  listener is needed. This is the same pattern rclone and gcloud use. The
  existing state and PKCE checks stay; only how the callback arrives changes.
- Also document the SSH tunnel option: `ssh -L 17842:127.0.0.1:17842 host`.

### Phase 6: Standard CLI output and module split

- Split `src/cli.rs` into `src/cli/{mod.rs, args.rs, commands.rs, output.rs,
  confirm.rs, service.rs}` with no behavior change. Do this first, as its own
  commit.
- Human-readable output by default:
  - `list` prints an aligned table with columns `ID  TIME  VICTIM  SHIP  VALUE
    STATUS  BULK`, built with `format!` widths and no new dependencies.
  - `status` and `show` print `key: value` lines.
  - `--json` keeps today's JSON schema exactly. `tests/cli.rs` already covers
    that schema.
- Exit codes stay the same: 0 ok, 1 error, 2 usage, 3 busy, 130 cancelled. The
  service is the exception: a clean stop exits 0.
- Update the README tables and the policy tests that read human output.

### Phase 7: Packaging and docs

- Add a hidden `ekmp generate completions <shell>` command (`clap_complete`)
  and a hidden `ekmp generate man <dir>` command (`clap_mangen`). Both run at
  packaging time, so no `build.rs` is needed.
- `scripts/package-linux.sh` builds a tarball containing `ekmp`, `install.sh`,
  `ekmp.service`, bash/zsh/fish completions, `ekmp.1` and the subcommand man
  pages, the README and the LICENSE. `install.sh` installs the completions
  under `$XDG_DATA_HOME` (bash-completion, zsh `site-functions`, fish
  `vendor_completions.d`) and the man pages under `~/.local/share/man/man1`.
- `install.sh` installs `~/.local/bin/ekmp` and
  `~/.config/systemd/user/ekmp.service`, then runs `systemctl --user
  daemon-reload`. It never enables or starts the service. `--uninstall`
  stops and disables the service, removes it, and keeps config and state.
- The README covers: install, `characters add` (desktop, `--paste`, SSH),
  `systemctl --user enable --now ekmp`, `loginctl enable-linger`, `journalctl
  --user -u ekmp`, file locations, and the API rules ekmp follows.
- Remove or archive `WINDOWS-VALIDATION-PLAN.md`, `packaging/windows`,
  `scripts/package-windows.ps1`, the Windows release job, and the Windows and
  macOS entries in `TODO.md`. This follows decision 0.2.
- Update `AGENTS.md`: Project context ("Linux CLI and refresh service; optional
  GUI"), Architecture (the new `cli/` modules and `persistence::paths`), and
  Persistence (the new file locations).

## Tests added along the way

- Refresh-interval floor, in both the parser and core.
- The ESI client sends `X-Compatibility-Date` and logs `Warning: 299`.
- The service loop, using `Core::in_memory` and a test backend: sleeps until
  due, exits 0 on cancel, idles with no characters, and never calls
  `Backend::post`.
- `--paste` callback parsing rejects a wrong `state`, a missing `code`, and a
  host other than the callback host.
- XDG path resolution with and without the `XDG_*` variables.
- `state.json` never contains `refresh_token`, by the same kind of sentinel
  test as the existing one.

## Risks

- **Compatibility-date switch:** an ESI response shape may change. Mitigation:
  pin the date, and run one manual live refresh and `show` before the release.
- **Re-authentication after the state split:** expected, given the AGENTS.md
  rule about compatibility. Mention it in the release notes.
- **Secret Service under linger:** without a desktop session, tokens fall back
  to `credentials.json` (0600). Say so in the README and in `ekmp status`.
- **Human output changes:** scripts must use `--json`. Call this out in the
  release notes.
