# TODO

## Development

### TODO-DEVELOPMENT-001 — Support Windows builds and application icons

**Status:** Pending.

Make the application compile, run, and present correctly on supported Windows
versions.

- Establish and document the Windows development build workflow, including the
  required Rust target and any native build tools.
- Build and manually test the native application on Windows, including EVE SSO
  browser authentication, local storage, and opening external links.
- Extract the supplied `windows/app-icon.ico` into a repository-owned Windows
  asset and embed it as the executable icon so Explorer, shortcuts, and pinned
  taskbar entries use the akmp artwork.
- Retain the embedded PNG viewport icon for the live window, Alt-Tab, and
  taskbar icon; verify it is shown correctly on Windows at common DPI scales.
- Add Windows-specific release/packaging documentation without changing the
  Linux development-launcher workflow.

**Acceptance criteria:** A clean Windows checkout can build and run akmp; its
executable and running-window surfaces display the supplied icon, and the
existing user-facing workflow works without Linux-only assumptions.

### TODO-DEVELOPMENT-003 — Test native credential storage on macOS and Windows

**Status:** Pending.

Manually integration-test refresh-token storage using macOS Keychain and
Windows Credential Manager.

- Authenticate a character and verify its refresh token is stored in the
  platform credential store rather than `akmp.json`.
- Restart the application and verify token retrieval and ESI requests succeed.
- Temporarily make the credential store unavailable or deny access; verify the
  application uses the same JSON fallback and persistent security warning as
  Linux.

**Acceptance criteria:** Both platforms use their native store during normal
operation and exhibit the documented fallback behavior when it fails.

### TODO-DEVELOPMENT-004 — Make status information understandable to users

**Status:** Pending.

Revise the Status and Activity text so it explains the result in terms a user
can act on, without requiring knowledge of ESI, zKillboard lookup categories,
or cached status states.

- Report only unreported killmail counts in user-facing status summaries; do
  not foreground existing/reported killmail counts.
- State how many killmails are protected and therefore excluded from bulk
  posting.
- Where practical, identify the authenticated character, authenticated
  corporation, or manually protected victim responsible for protection.
- Keep detailed lookup diagnostics available for troubleshooting, but separate
  them from the primary user-facing status summary.

**Acceptance criteria:** After loading killmails, a user can understand how
many can be bulk posted, how many are protected and why, and whether any action
is needed, without interpreting internal status terminology.



### TODO-DEVELOPMENT-002 — Rename the application to EVE Killmail Publisher

**Status:** Pending.

The current user-facing name is **EVE Killmail Publisher**, but the project
still uses `akmp` internally and in development/release assets. Decide whether
to rename this repository or create a new repository, then update the complete
application identity consistently.

- Rename the Cargo package, executable, user agent, window/application ID, and
  all source-code references to `akmp` where appropriate.
- Decide whether existing configuration under `~/.config/akmp/` should be
  migrated to the new storage location or intentionally left behind, and
  document the decision.
- Update README instructions, launcher scripts, desktop-entry templates,
  icons/assets, release metadata, CI, and packaging names.
- Update the repository URL, issue links, release artifacts, and any new
  repository transition documentation if a separate repository is created.
- Search the entire repository for stale `akmp` references and verify the
  branded window title, subtitle, executable, launcher, and installed desktop
  entry all use the final name.

**Acceptance criteria:** A clean checkout of the renamed project builds an
executable and packaged desktop entry branded as EVE Killmail Publisher, with
documented handling of existing user configuration and no unintended stale
`akmp` identity references.

## Release

### TODO-RELEASE-001 — Replace the development Wayland launcher

**Status:** Pending before the first public release.

The current KDE/Wayland launcher is intentionally development-only: it writes
`~/.local/share/applications/akmp.desktop` with absolute paths to this checkout
and `target/debug/akmp`.

Before release:

- Remove `scripts/setup-dev-launcher.sh`.
- Remove `assets/linux/akmp.desktop.in`.
- Replace the development-launcher section in `README.md` with release
  installation/packaging instructions.
- Add release packaging that installs a production `akmp.desktop` entry and
  appropriately named Linux `hicolor` icon assets.
- Confirm the production desktop-entry filename remains `akmp.desktop`, so it
  matches eframe's Wayland application ID.
- Keep `assets/app-icon.png` and the eframe viewport icon unless the release
  packaging approach deliberately replaces their runtime use; they remain
  useful for X11 and future Windows support.

**Acceptance criteria:** A package-installed release shows the `akmp` icon in
KDE/Wayland launchers and taskbars without any checkout-specific paths or
development setup script.
