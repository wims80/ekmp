#!/usr/bin/env bash
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
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
bin_dir="$HOME/.local/bin"
applications_dir="$data_dir/applications"
icons_dir="$data_dir/icons/hicolor"
binary_path="$bin_dir/ekmp"
desktop_path="$applications_dir/ekmp.desktop"
icon_path="$icons_dir/256x256/apps/ekmp.png"

if "$uninstall"; then
    rm -f -- "$binary_path" "$desktop_path" "$icon_path"
    refresh_desktop_metadata
    printf 'EVE Killmail Publisher was removed. Settings in %s were kept.\n' \
        "${XDG_CONFIG_HOME:-$HOME/.config}/ekmp"
    exit 0
fi

for required_file in ekmp ekmp.png ekmp.desktop.in; do
    if [[ ! -f "$script_dir/$required_file" ]]; then
        printf 'Release archive is missing %s. Extract the complete archive first.\n' \
            "$required_file" >&2
        exit 1
    fi
done

mkdir -p -- "$bin_dir" "$applications_dir" "$(dirname -- "$icon_path")"
install -m 755 -- "$script_dir/ekmp" "$binary_path"
install -m 644 -- "$script_dir/ekmp.png" "$icon_path"

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

printf 'Installed EVE Killmail Publisher. Launch it from the application menu or run %s.\n' \
    "$binary_path"
