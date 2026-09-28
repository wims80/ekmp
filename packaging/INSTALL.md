# EVE Killmail Publisher installation

## Linux

Extract the complete archive and run `./install.sh`. It installs the program for the current user without administrator access. `ekmp --help` opens the CLI help and `ekmp gui` opens the desktop interface; the installed desktop launcher runs `ekmp gui`. Run `./install.sh --uninstall` from the same archive to remove the program, launcher, and icon while retaining settings and caches.

`ekmp-refresh.service` is an opt-in systemd user-service template. If periodic refreshes are wanted, copy it to `~/.config/systemd/user/`, then run `systemctl --user daemon-reload` and `systemctl --user enable --now ekmp-refresh.service`. It refreshes cached data only and never posts killmails. Installation does not enable the service.

## Windows

Extract the complete ZIP. Run `launch-gui.cmd` for the GUI, or run `ekmp.exe` from a terminal for the CLI. The executable is not code signed, so Windows may show a SmartScreen warning. Compare the archive with the release `SHA256SUMS` before running it.

For periodic refreshes, create an At log on Task Scheduler task under the same Windows account that authenticated the characters. Its program is the extracted `ekmp.exe` and its arguments are `service run`. Do not use another account or elevation because credentials and state are user-scoped. The task is never created automatically and never posts killmails.

The program never needs an EVE client secret. Keep `ekmp.json`, refresh tokens, and authorization URLs private.
