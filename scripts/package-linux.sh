#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf 'Usage: %s VERSION [OUTPUT_DIRECTORY]\n' "${0##*/}"
}

if [[ $# -lt 1 || $# -gt 2 ]]; then
    usage >&2
    exit 2
fi

version="$1"
output_dir="${2:-dist}"
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
binary="$repo_root/target/release/ekmp"
archive_name="ekmp-v${version}-x86_64-unknown-linux-gnu.tar.gz"
archive_root="ekmp-v${version}-x86_64-unknown-linux-gnu"

if [[ ! -x "$binary" ]]; then
    printf 'Release binary not found: %s\nRun cargo build --release first.\n' "$binary" >&2
    exit 1
fi

mkdir -p -- "$output_dir"
output_dir="$(cd -- "$output_dir" && pwd -P)"
archive_path="$output_dir/$archive_name"
if [[ -e "$archive_path" ]]; then
    printf 'Refusing to overwrite existing archive: %s\n' "$archive_path" >&2
    exit 1
fi

staging_dir="$(mktemp -d)"
trap 'rm -rf -- "$staging_dir"' EXIT
package_dir="$staging_dir/$archive_root"
mkdir -p -- "$package_dir"

install -m 755 -- "$binary" "$package_dir/ekmp"
install -m 755 -- "$repo_root/packaging/linux/install.sh" "$package_dir/install.sh"
sed 's|@BINDIR@|%h/.local/bin|' "$repo_root/packaging/linux/ekmp.service.in" > "$package_dir/ekmp.service"
chmod 644 -- "$package_dir/ekmp.service"
install -m 644 -- "$repo_root/packaging/INSTALL.md" "$package_dir/README.md"
install -m 644 -- "$repo_root/LICENSE" "$package_dir/LICENSE"

# Release binaries are built with the gui feature, so the archive includes its launcher.
install -m 644 -- "$repo_root/packaging/linux/ekmp.desktop.in" "$package_dir/ekmp.desktop.in"
install -m 644 -- "$repo_root/assets/app-icon.png" "$package_dir/ekmp.png"

mkdir -p -- "$package_dir/completions" "$package_dir/man"
"$binary" generate completions bash > "$package_dir/completions/ekmp.bash"
"$binary" generate completions zsh > "$package_dir/completions/_ekmp"
"$binary" generate completions fish > "$package_dir/completions/ekmp.fish"
"$binary" generate man "$package_dir/man"
gzip -9 --no-name -- "$package_dir"/man/*.1

tar --create --gzip --file "$archive_path" --directory "$staging_dir" "$archive_root"
printf 'Created %s\n' "$archive_path"
