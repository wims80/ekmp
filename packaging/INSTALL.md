# EVE Killmail Publisher installation

## Linux

Extract the complete archive and run `./install.sh`. It installs for the current user without administrator access: `ekmp` in `~/.local/bin`, the `ekmp.service` systemd user unit, bash/zsh/fish completions, man pages, and a desktop launcher for `ekmp gui`. Run `ekmp --help` or `man ekmp` to get started, and `ekmp characters add` to sign in (`--paste` on a headless host).

The refresh service is opt-in: `systemctl --user enable --now ekmp`. It refreshes cached data only and never posts killmails. View its logs with `journalctl --user -u ekmp`, and run `loginctl enable-linger "$USER"` to keep it running while logged out. Installation never enables or starts it.

Run `./install.sh --uninstall` from the same archive to stop the service and remove every installed file while retaining settings, state, and caches.

The program never needs an EVE client secret. Keep `~/.local/state/ekmp/`, refresh tokens, and authorization URLs private.
