## Project context

- The user-facing application name is **EVE Killmail Publisher**.
- The lowercase shorthand and technical identifier is `ekmp`; the Cargo
  package, executable, application ID, storage paths, and platform assets use
  that identifier.
- Repository creation and GitHub identity work remain tracked in `TODO.md`.
- This is a Rust application for Linux. The default build is the CLI and its
  refresh service; the `eframe`/`egui` GUI is an opt-in `gui` feature that
  release builds enable.
- EVE data comes from ESI, authentication uses EVE SSO with PKCE, and killmails
  are submitted to zKillboard.
- The EVE client ID is a public application identifier. A client secret must
  never be embedded, stored, or requested from users.

## Product invariants

- Killmails must never be submitted automatically.
- Every submission requires an explicit user action.
- Bulk submission includes only killmails confirmed as unreported.
- Bulk submission must never include protected killmails, including if
  protection changes while a confirmation dialog is open.
- Protected killmails may only be submitted through their individual
  `Post anyway` action.
- Authenticated characters and their corporations are automatically protected.
- Users may manually protect additional victim characters and corporations.
- Protected killmails are hidden by default; the persisted
  `Show protected killmails` preference reveals them.
- Reported killmails are not displayed in the recent-killmail list.
- Full reported killmail records are not persisted. Compact reported-ID cache
  entries are retained to avoid redundant API requests.
- Explicit successful submissions are shown in a session-only results panel
  and are not persisted.
- Unknown or expired-negative zKillboard statuses are refreshed for cached
  killmails at startup.
- Use cached status information and request spacing to avoid unnecessarily
  hammering ESI or zKillboard. Refresh intervals are at least
  `MIN_REFRESH_INTERVAL_SECS` (5 minutes) wherever they enter.

## Architecture

- `src/core/` owns GUI-independent application state, typed commands and
  snapshots, operation coordination, persistence, credential operations,
  refresh scheduling, cross-process locking, and submission revalidation.
  `mod.rs` retains the `Core` entry point, private fields, explicit re-exports,
  and event registration and emission. Private modules split responsibilities:
  `types.rs` owns shared errors, events, snapshots, selections, and results;
  `store.rs` owns persistence, construction, initialization, operation and
  service guards, snapshots, status summaries, and simple persisted mutations;
  an operation works on an owned copy of the store that becomes visible to
  other readers only when persisted;
  `characters.rs` owns authentication, removal, and refresh-token migration;
  `protection.rs` owns killmail and victim protection and the show-protected
  preference; `refresh.rs` owns refresh operations, scheduling, backoff,
  affiliations, and ESI loading; `status.rs` owns shared zKillboard status
  refresh, pagination, query caching, evidence reconciliation, reported-record
  pruning, and source timestamps; `posting.rs` owns confirmation, submission
  revalidation, skipped results, and session reports; `timing.rs` owns
  cancellation, waits, API cooldowns, and durable zKillboard request spacing.
  Integration errors convert into `CoreError` at this boundary. Tests live alongside their owning modules, with shared backend and
  store fixtures in test-only `test_support.rs`.
- `src/cli/` owns the command-line interface. `mod.rs` owns `run`, command
  dispatch, cancellation handling, exit codes, and offline-scenario core
  construction shared with the GUI. `args.rs` owns Clap definitions and value
  validation; `commands.rs` owns the character, posting, protection, and
  configuration commands; `prompt.rs` owns interruptible terminal input and
  confirmation; `output.rs` owns `Output`, which pairs each result's JSON with
  its human-readable text; `text.rs` owns table and value formatting. The
  `--json` schema is the stable script interface; text output may change.
  The hidden `generate` command writes shell completions (`clap_complete`)
  and man pages (`clap_mangen`) for packaging, before any core is created.
  `service.rs` owns the foreground refresh service run by the `ekmp.service`
  systemd user unit: it only refreshes, waits until a refresh is due (checking
  at least once a minute), logs one line per cycle to stderr for journald,
  waits rather than exits without characters, and exits 0 on SIGINT or
  SIGTERM.
- `src/clock.rs` owns system time, HTTP-date conversion, and UTC formatting.
- `src/app/` owns the optional GUI shell and its launcher. It renders core
  snapshots, GUI-only textures and expansion state, and polls shared state
  without writing a stale snapshot back to storage.
- `src/app/ui/` owns egui rendering and user interactions: `mod.rs` owns the
  app frame and dispatches component actions, `theme.rs` owns shared visual
  styling, `components.rs` owns shared widgets, `dialogs.rs` owns confirmation
  dialogs, `sidebar.rs` owns sidebar rendering, and `killmail.rs` owns
  killmail-card and detail rendering. Components return action values rather
  than writing through out-parameters.
- `src/app/operations.rs` runs core operations and snapshot polling on background
  threads. Each operation returns a completion closure that updates GUI
  presentation from its result.
- `src/app/worker.rs` owns the GUI image-loading worker.
- `src/killmail.rs` owns killmail visibility, reporting status, protection,
  and submission policy.
- `src/integrations/` owns external API integrations: EVE SSO authentication
  (`auth.rs`; its `AuthFlow` receives the callback on the loopback listener or
  from a pasted redirect URL, validating the callback URL and OAuth state),
  ESI data access, EVE image-service portraits and logos, and zKillboard lookup
  and submission. Its single backend interface separates the live adapters from
  the feature-gated offline simulator used by workers and UI tests. `mod.rs`
  owns the typed `ApiError`; only its `Other` kind is recoverable, and callers
  must classify errors by kind, never by message text. `http.rs` owns the
  shared HTTP clients, user agent, transport errors, `Retry-After` parsing, and
  the per-backend `ApiLog` through which adapters report API cooldowns and
  once-per-route deprecation warnings; core drains it with
  `absorb_api_observations`.
- `src/integrations/esi/` owns the ESI adapter: `client.rs` owns the `Esi`
  client with cached GETs, POSTs, rate-limit and deprecation observation, and
  the pinned `X-Compatibility-Date` (`COMPATIBILITY_DATE` in `mod.rs`; bump it
  only after checking every route against `types.rs`), `killmails.rs`
  assembles killmails, `universe.rs` resolves EVE identities and protected
  victims, `market.rs` estimates values, and `types.rs` contains private
  response DTOs.
- `dev/scenarios/` owns synthetic JSON scenarios for offline development. The
  `dev-tools` feature enables scenario launch in CLI-only and GUI builds;
  eframe inspection is enabled only with the `gui` feature. Live runs must
  never expose the inspection interface.
- `src/models.rs` contains persisted and domain models. Each cached
  killmail's zKillboard evidence is a single `ZkillStatus`.
- `src/persistence/mod.rs` owns private file-permission helpers;
  `paths.rs` owns the XDG config, state, and cache directories.
- `src/persistence/secrets.rs` owns refresh-token storage in the system
  credential store (Secret Service on Linux). When it fails, tokens fall back
  to `credentials.json`; keep its work off the UI thread.
- `src/persistence/storage.rs` owns `StorePaths`, splitting `Store` into its
  config, state, and credentials files, atomic saving with restrictive Unix
  file permissions, fail-safe handling of unreadable files, and the operation
  and service locks beside the state file.
- `src/persistence/image_cache.rs` owns the local cache for public EVE character
  portraits and corporation logos.
- `src/persistence/esi_cache.rs` owns the local SQLite cache for cacheable ESI
  GET responses, including expiry and conditional-request metadata.
- `packaging/linux/` owns the release installer, the `ekmp.service` systemd
  user unit, and the desktop launcher. `install.sh` installs per user under
  `~/.local` and `~/.config/systemd/user`, never enables or starts the
  service, and its `--uninstall` stops the service and keeps settings, state,
  and caches. `scripts/package-linux.sh` assembles the release archive,
  generating completions and man pages from the release binary.
- Keep blocking HTTP and sleeps off the egui UI thread.
- Keep submission-policy functions centralized and covered by tests.
- When architectural boundaries, module ownership, or important paths change,
  update this Architecture section in the same change. Do not leave
  `AGENTS.md` describing an obsolete design.

## Persistence

- `Store` is persisted as three files by `persistence/storage.rs`:
  preferences in `$XDG_CONFIG_HOME/ekmp/config.toml`, everything else in
  `$XDG_STATE_HOME/ekmp/state.json`, and OAuth refresh-token fallbacks in
  `$XDG_STATE_HOME/ekmp/credentials.json`. `Store`'s serde form is the state
  file, so preference fields and refresh tokens are `#[serde(skip)]`.
- `config.toml` is hand-editable: unknown keys are rejected, and it is
  rewritten only when a setting's value changes, preserving user comments.
- `credentials.json` exists only while the system credential store has failed
  for some character, and must be treated as sensitive.
- Persistence compatibility is not currently required because the application
  is under heavy development and has one user.
- Do not add compatibility aliases or migrations unless explicitly requested.
- Session-only UI state must not be added to `Store`.

## Working rules

- Keep changes focused on the requested task.
- Preserve existing public behavior unless the task requires changing it.
- Do not add production dependencies without asking first.
- Do not modify generated files directly.
- Do not commit changes unless explicitly asked.
- Preserve the product invariants above when changing UI layout or refactoring.
- When behavior changes, update the relevant policy tests and README
  documentation.
- Use the terminology “protected victim,” “eligible for bulk posting,” and
  “reported” consistently.

## Rust conventions

- Follow the existing architecture and naming conventions.
- Prefer clear, idiomatic Rust over clever abstractions.
- Avoid unnecessary cloning and broad `allow` attributes.
- Add or update tests when behavior changes.

## Verification

After changing Rust code, run:

- `cargo fmt --check`
- `cargo check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features`

If a command cannot run, explain why in the final response.

## Definition of done

- The requested behavior is implemented.
- Relevant tests pass.
- Formatting and lint checks pass.
- The final response summarizes changed files and any remaining risks.
