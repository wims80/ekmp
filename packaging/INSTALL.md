# EVE Killmail Publisher installation

## Linux

Extract the complete Linux archive and run `./install.sh`. It installs the
program only for the current user and does not require administrator access.
Run `./install.sh --uninstall` from the same extracted archive to remove the
program, launcher, and icon. It intentionally keeps your configuration and
cached data in `~/.config/ekmp`.

## Windows

Extract the complete ZIP and run `ekmp.exe`. The executable is not code signed,
so Windows may show a SmartScreen warning. Compare the downloaded archive with
the `SHA256SUMS` file on the GitHub Release before running it.

The application never needs an EVE client secret. Keep `ekmp.json`, refresh
tokens, and authorization URLs private.
