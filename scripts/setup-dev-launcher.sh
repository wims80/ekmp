#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
template="$repo_root/assets/linux/akmp.desktop.in"
desktop_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
desktop_file="$desktop_dir/akmp.desktop"

mkdir -p "$desktop_dir"
temporary_file="$(mktemp "$desktop_dir/.akmp.XXXXXX.desktop")"
trap 'rm -f "$temporary_file"' EXIT

while IFS= read -r line || [[ -n "$line" ]]; do
    printf '%s\n' "${line//@REPO_ROOT@/$repo_root}"
done < "$template" > "$temporary_file"

if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$temporary_file"
fi

mv "$temporary_file" "$desktop_file"
trap - EXIT

if command -v kbuildsycoca6 >/dev/null; then
    kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
elif command -v kbuildsycoca5 >/dev/null; then
    kbuildsycoca5 --noincremental >/dev/null 2>&1 || true
fi

printf 'Development launcher updated: %s\n' "$desktop_file"
