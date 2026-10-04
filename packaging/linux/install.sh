#!/usr/bin/env bash
# Installs EVE Killmail Publisher for the current user under ~/.local.
# It never enables or starts the refresh service.
set -euo pipefail

usage() {
    printf 'Usage: %s [--uninstall]\n' "${0##*/}"
}

refresh_desktop_metadata() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$applications_dir" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache --force "$icons_dir" >/dev/null 2>&1 || true
    fi
    if command -v kbuildsycoca6 >/dev/null 2>&1; then
        kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
    elif command -v kbuildsycoca5 >/dev/null 2>&1; then
        kbuildsycoca5 --noincremental >/dev/null 2>&1 || true
    fi
}

# Runs systemctl --user when a user service manager is reachable.
user_systemctl() {
    command -v systemctl >/dev/null 2>&1 && systemctl --user "$@" >/dev/null 2>&1
}

case "${1:-}" in
    "") uninstall=false ;;
    --uninstall) uninstall=true ;;
    -h|--help)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
bin_dir="$HOME/.local/bin"
unit_dir="$config_home/systemd/user"
applications_dir="$data_dir/applications"
icons_dir="$data_dir/icons/hicolor"
man_dir="$data_dir/man/man1"

binary_path="$bin_dir/ekmp"
unit_path="$unit_dir/ekmp.service"
desktop_path="$applications_dir/ekmp.desktop"
icon_path="$icons_dir/256x256/apps/ekmp.png"
bash_completion_path="$data_dir/bash-completion/completions/ekmp"
zsh_completion_path="$data_dir/zsh/site-functions/_ekmp"
fish_completion_path="$data_dir/fish/vendor_completions.d/ekmp.fish"

if "$uninstall"; then
    if [[ -f "$unit_path" ]]; then
        user_systemctl disable --now ekmp.service || true
    fi
    rm -f -- "$binary_path" "$unit_path" "$desktop_path" "$icon_path" \
        "$bash_completion_path" "$zsh_completion_path" "$fish_completion_path" \
        "$man_dir"/ekmp.1.gz "$man_dir"/ekmp-*.1.gz
    user_systemctl daemon-reload || true
    refresh_desktop_metadata
    printf 'EVE Killmail Publisher was removed. Settings in %s and data in %s were kept.\n' \
        "$config_home/ekmp" "${XDG_STATE_HOME:-$HOME/.local/state}/ekmp"
    exit 0
fi

for required_file in ekmp ekmp.service completions/ekmp.bash completions/_ekmp \
    completions/ekmp.fish man/ekmp.1.gz; do
    if [[ ! -f "$script_dir/$required_file" ]]; then
        printf 'Release archive is missing %s. Extract the complete archive first.\n' \
            "$required_file" >&2
        exit 1
    fi
done

install -D -m 755 -- "$script_dir/ekmp" "$binary_path"
install -D -m 644 -- "$script_dir/ekmp.service" "$unit_path"
install -D -m 644 -- "$script_dir/completions/ekmp.bash" "$bash_completion_path"
install -D -m 644 -- "$script_dir/completions/_ekmp" "$zsh_completion_path"
install -D -m 644 -- "$script_dir/completions/ekmp.fish" "$fish_completion_path"
mkdir -p -- "$man_dir"
install -m 644 -- "$script_dir"/man/*.1.gz "$man_dir/"
user_systemctl daemon-reload || true

if [[ -f "$script_dir/ekmp.desktop.in" && -f "$script_dir/ekmp.png" ]]; then
    install -D -m 644 -- "$script_dir/ekmp.png" "$icon_path"
    mkdir -p -- "$applications_dir"
    temporary_desktop="$(mktemp "$applications_dir/.ekmp.XXXXXX.desktop")"
    trap 'rm -f -- "$temporary_desktop"' EXIT
    printf -v escaped_exec '%q' "$binary_path"
    sed "s|@EXEC_PATH@|$escaped_exec|" "$script_dir/ekmp.desktop.in" > "$temporary_desktop"
    if command -v desktop-file-validate >/dev/null 2>&1; then
        desktop-file-validate "$temporary_desktop"
    fi
    mv -- "$temporary_desktop" "$desktop_path"
    trap - EXIT
    refresh_desktop_metadata
fi

printf 'Installed EVE Killmail Publisher as %s.\n\n' "$binary_path"
printf 'Next steps:\n'
printf '  ekmp characters add                    # sign in (add --paste on a headless host)\n'
printf '  systemctl --user enable --now ekmp     # optional: refresh in the background\n'
printf '  journalctl --user -u ekmp              # service logs\n'
if user_systemctl is-active ekmp.service; then
    printf '\nThe refresh service is running the previous version; restart it with\n'
    printf '  systemctl --user restart ekmp\n'
fi
if [[ -f "$unit_dir/ekmp-refresh.service" ]]; then
    printf '\nThe old ekmp-refresh.service unit is still installed. Replace it with:\n'
    printf '  systemctl --user disable --now ekmp-refresh.service\n'
    printf '  rm %q\n' "$unit_dir/ekmp-refresh.service"
    printf '  systemctl --user enable --now ekmp\n'
fi
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) printf '\nAdd %s to PATH to run ekmp by name.\n' "$bin_dir" ;;
esac
