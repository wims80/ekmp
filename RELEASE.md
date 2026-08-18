# Release runbook

This runbook covers the `v0.2.0` release of EVE Killmail Publisher.
It ships x86-64 Linux (glibc 2.35+) and x86-64 Windows builds.

## One-time GitHub setup

Complete these steps in the GitHub web interface before making the repository
public:

1. Confirm the full Git history contains no tokens, local configuration, or
   client secrets. The EVE client ID is public; a client secret must never be
   committed.
2. Add the repository description and publish it. Do not add collaborators:
   a public repository lets people submit pull requests without repository
   write access.
3. Enable Issues, Actions, Dependabot alerts and security updates, secret
   scanning, and private vulnerability reporting. Disable Discussions, Wiki,
   Projects, Packages, and other unused features.
4. Protect `main` from force-pushes and deletion, require CI for pull-request
   merges, and retain an owner bypass for direct maintenance.
5. Reauthenticate the local GitHub CLI before using it to create releases.

Standard GitHub-hosted Linux and Windows runners are free for public
repositories. The release workflow creates a draft, so publishing remains a
deliberate manual action.

## Artifact contract

Each release attaches exactly these files:

- `ekmp-vVERSION-x86_64-unknown-linux-gnu.tar.gz`
- `ekmp-vVERSION-x86_64-pc-windows-msvc.zip`
- `SHA256SUMS`

The Linux archive contains the executable, `install.sh`, desktop-entry
template, `hicolor` icon, installation notes, and MIT license. `install.sh`
installs only for the current user under `~/.local`; `--uninstall` removes the
program assets and deliberately preserves configuration. The Windows ZIP
contains `ekmp.exe`, installation notes, and the license. Neither archive
contains a default configuration file.

GitHub's automatically generated source ZIP and tarball are source code, not
runnable application downloads.

## Creating a release

1. Finish the release changes on `main` and wait for CI to pass.
2. Set the Cargo package version to the intended numeric version and ensure the
   Rust toolchain, lockfile, README, and release notes are current.
3. Run the full local verification suite:

   ```sh
   cargo fmt --check
   cargo check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   ```

4. Create and push an annotated tag named `vVERSION`. The release workflow
   verifies that the tag and Cargo version agree, builds both targets, creates
   the archives and checksums, and opens a draft pre-release.
5. Download the draft artifacts and complete the smoke tests below. Verify
   each archive against `SHA256SUMS`.
6. Write concise release notes: supported platforms, glibc 2.35 floor,
   unsigned-Windows warning, manual update model, known limitations, and the
   GitHub Issues link. Publish the draft only after the smoke tests pass.
7. Share the GitHub Release URL with the test group and monitor GitHub Issues.

## Smoke-test checklist

- On Linux, install the exact downloaded archive in a clean user account and
  confirm the launcher, GNOME/KDE taskbar icon, direct execution, reinstall,
  and uninstall behavior. Confirm uninstall preserves `~/.config/ekmp`.
- On Windows 10 or newer, run the exact downloaded `ekmp.exe`, confirm the
  executable and window icons, browser authentication callback, persistence,
  Credential Manager behavior, and external links.
- On both platforms, authenticate, load cached and fresh killmails, confirm
  protected-victim visibility, status refresh, and character removal.
- Confirm that posting is always explicitly initiated, bulk posting excludes
  protected victims even after its confirmation dialog opens, and protected
  killmails require the individual `Post anyway` action.
- Test the Linux Secret Service fallback warning if practical. Never use or
  disclose a real refresh token in test artifacts or issue reports.

## Corrections and rollback

Delete and recreate an unpublished draft if its artifacts are wrong. Never
silently replace assets on a published release. Mark a bad release as
superseded in its notes and publish a new patch tag such as `v0.1.1`.
