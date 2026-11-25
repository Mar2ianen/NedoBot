#!/usr/bin/env bash
# Единый список исполняемых файлов задаёт installer.
set -euo pipefail

case "${1:-}" in
    pack)
        source_dir="${2:?missing build directory}"
        bundle="${3:?missing bundle directory}"
        script_dir="$(dirname -- "${BASH_SOURCE[0]}")"
        installer="$script_dir/install_release_binaries.sh"
        names="$(bash "$installer" --list-binaries)"
        mkdir -- "$bundle"
        while IFS= read -r binary; do
            test -x "$source_dir/$binary" || { echo "missing build executable: $binary" >&2; exit 1; }
            install -m 0755 -- "$source_dir/$binary" "$bundle/$binary"
        done <<< "$names"
        install -m 0755 -- "$installer" "${BASH_SOURCE[0]}" "$bundle/"
        install -m 0755 -- "$script_dir/set_privacy_mode_observe.sh" "$bundle/"
        ( cd "$bundle" && sha256sum -- * > SHA256SUMS )
        bash "$bundle/release_bundle.sh" verify "$bundle"
        ;;
    verify)
        cd -- "${2:?missing bundle directory}"
        sha256sum --strict --check --status SHA256SUMS
        names="$(bash install_release_binaries.sh --list-binaries)"
        expected="$(printf '%s\n' "$names" install_release_binaries.sh release_bundle.sh set_privacy_mode_observe.sh | LC_ALL=C sort)"
        measured="$(cut -c 67- SHA256SUMS | LC_ALL=C sort)"
        actual="$(find . -mindepth 1 -maxdepth 1 ! -name SHA256SUMS ! -name BUILD_INFO.txt -printf '%f\n' | LC_ALL=C sort)"
        if [[ "$expected" != "$measured" || "$expected" != "$actual" ]]; then
            echo "release artifact does not match installer inventory" >&2
            exit 1
        fi
        while IFS= read -r binary; do
            chmod +x -- "$binary"
        done <<< "$expected"
        ;;
    *)
        echo "usage: release_bundle.sh pack <build-dir> <new-bundle-dir> | verify <bundle-dir>" >&2
        exit 2
        ;;
esac
