## Project context

- The user-facing application name is **EVE Killmail Publisher**.
- The Cargo package, executable, application ID, storage paths, and some assets
  still use the internal name `akmp`.
- Renaming the remaining `akmp` references is tracked in `TODO.md`; do not
  perform that migration incidentally.
- This is a native Rust desktop application built with `eframe`/`egui`.
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
  hammering ESI or zKillboard.

## Architecture

- `src/app/mod.rs` owns application state, operations, event handling, and
  persistence coordination.
- `src/app/ui.rs` owns egui rendering and user interactions.
- `src/app/worker.rs` owns blocking background work and worker events.
- `src/killmail.rs` owns killmail visibility, reporting status, protection,
  ordering, and submission policy.
- `src/auth.rs`, `src/esi.rs`, and `src/zkill.rs` are API-specific modules.
- `src/models.rs` contains persisted and domain models.
- `src/storage.rs` owns local configuration loading and saving.
- Keep blocking HTTP and sleeps off the egui UI thread.
- Keep submission-policy functions centralized and covered by tests.
- When architectural boundaries, module ownership, or important paths change,
  update this Architecture section in the same change. Do not leave
  `AGENTS.md` describing an obsolete design.

## Persistence

- Local state is currently stored in `~/.config/akmp/akmp.json`.
- It contains OAuth refresh tokens and must be treated as sensitive.
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
