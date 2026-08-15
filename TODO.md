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
