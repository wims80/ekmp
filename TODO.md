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

### Windows

#### TODO-DEVELOPMENT-001 — Support Windows builds and application icons

**Status:** Pending manual validation.

Windows builds use the default `x86_64-pc-windows-msvc` Rust target and Visual
Studio Build Tools with the Desktop development with C++ workload and a Windows
SDK. The build locates the Windows resource compiler to embed
`assets/windows/app-icon.ico` in `ekmp.exe`; `assets/app-icon.png` remains the
viewport icon. Local configuration is stored in `%APPDATA%\ekmp\ekmp.json`,
which avoids the Unix-only `HOME` assumption.

Verified 2026-08-15: `cargo check`, Clippy, tests, and a release build passed
on Windows; the release executable contains the supplied icon. Before closing
this TODO, manually test EVE SSO browser authentication, local-state
persistence, external-link opening, and the live viewport icon at common DPI
scales. Credential Manager verification remains tracked separately in
TODO-DEVELOPMENT-003-WINDOWS.

**Acceptance criteria:** A clean Windows checkout can build and run ekmp; its
executable and running-window surfaces display the supplied icon, and the
existing user-facing workflow works without Linux-only assumptions.

#### TODO-DEVELOPMENT-003-WINDOWS — Test Credential Manager storage

**Status:** Complete.

Windows Credential Manager storage is exercised by the opt-in
`windows_credential_manager_round_trip` test. It creates, reads, and deletes a
uniquely named temporary credential without accessing an EVE refresh token;
run it from an interactive, signed-in PowerShell session.

Verified 2026-08-15: the persisted record for Miws Vokan has no JSON
refresh-token fallback, confirming its refresh token was accepted by the
system credential store. The application already covers the storage-failure
branch with `failed_token_migration_keeps_the_json_fallback_and_warns`.

Verified 2026-08-15: interactive Windows testing confirmed Credential Manager
storage and the application workflow. The opt-in round-trip test remains
available for future environment checks.

**Acceptance criteria:** Windows uses Credential Manager during normal
operation and exhibits the documented fallback behavior when it fails.

### macOS

#### TODO-DEVELOPMENT-005 — Support macOS builds and application icons

**Status:** Pending.

Make the application compile, run, and present correctly on supported macOS
versions.

- Establish and document the macOS development build workflow, including the
  required Rust target, Xcode command-line tools, and app-bundle tooling.
- Build and manually test the native application on macOS, including EVE SSO
  browser authentication, local storage, and opening external links.
- Use the clean-edge transparent 1024×1024 PNG as the master artwork.
- Generate the standard macOS icon sizes (16, 32, 128, 256, 512, and 1024
  pixels) and package them as an asset catalog or multi-resolution `.icns`
  file.
- Add the generated macOS icon resource to the future application bundle and
  configure its bundle icon metadata.
- Verify the icon in the Finder, Dock, application switcher, and a running
  native window at common Retina and non-Retina scales.
- Review the 16×16 and 32×32 variants for legibility of the diagonal slash and
  ship silhouette; provide simplified artwork if they are unclear.
- Add macOS-specific release/packaging documentation without changing the
  Linux development-launcher workflow.

**Acceptance criteria:** A clean macOS checkout can build and run ekmp; its
application bundle and running-window surfaces display the supplied icon, and
the existing user-facing workflow works without Linux-only assumptions.

#### TODO-DEVELOPMENT-003-MACOS — Test Keychain storage

**Status:** Pending.

Manually integration-test refresh-token storage using macOS Keychain.

- Authenticate a character and verify its refresh token is stored in Keychain
  rather than `ekmp.json`.
- Restart the application and verify token retrieval and ESI requests succeed.
- Temporarily make Keychain unavailable or deny access; verify the application
  uses the JSON fallback and persistent security warning.

**Acceptance criteria:** macOS uses Keychain during normal operation and
exhibits the documented fallback behavior when it fails.

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

### Windows

No pending Windows release TODOs.

### macOS

No pending macOS release TODOs.
