# TODO

## Development

### Generic

#### TODO-DEVELOPMENT-002 — Rename the application to EVE Killmail Publisher

**Status:** Complete.

The local application identity has been renamed to **EVE Killmail Publisher**,
with the lowercase shorthand and technical identifier `ekmp`. The Cargo
package, executable, user agent, window/application ID, storage location,
development launcher, desktop-entry template, and documentation use the new
identity.

Configuration is stored under `~/.config/ekmp/ekmp.json` on Linux and
`%APPDATA%\ekmp\ekmp.json` on Windows.

Verified 2026-08-15: the repository remote is
`https://github.com/wims80/ekmp`; repository-owned identity references use
`ekmp` or EVE Killmail Publisher; and `cargo build` produces the `ekmp`
executable. Direct GitHub reachability could not be checked from the local
sandbox because DNS resolution for `github.com` is unavailable.

**Acceptance criteria:** A clean checkout of the new repository builds an
`ekmp` executable and desktop entry branded as EVE Killmail Publisher, with
repository links and release artifacts using the final identity.

#### TODO-DEVELOPMENT-004 — Make status information understandable to users

**Status:** Complete.

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

### Linux

No pending Linux development TODOs.

## Release

### Generic

No pending generic release TODOs.

### Linux

#### TODO-RELEASE-001 — Replace the development Wayland launcher

**Status:** Implementation complete; pending manual release validation.

The checkout-specific development launcher has been removed. Release archives
now contain a user-local installer that installs `ekmp` to `~/.local/bin`, a
production `ekmp.desktop` launcher, and an `ekmp` hicolor icon. The installed
desktop entry uses the production executable path and retains the
`ekmp.desktop` filename required by eframe's Wayland application ID.

Before the first published release, manually verify the archive on GNOME and
KDE/Wayland, including launcher and taskbar icon resolution, installation,
reinstallation, and uninstall preservation of `~/.config/ekmp`.

**Acceptance criteria:** A package-installed release shows the `ekmp` icon in
KDE/Wayland launchers and taskbars without any checkout-specific paths or
development setup script.

