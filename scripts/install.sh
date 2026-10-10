#!/bin/sh
# Installs a jackdaw release for the current user, or removes it.
#
#   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jbuehler23/jackdaw/releases/latest/download/install.sh | sh
#
# Pass options after `sh -s --`. Run with --help for the list.
set -eu

REPO="jbuehler23/jackdaw"
MARKER="# jackdaw-installer"

say() { printf 'jackdaw-install: %s\n' "$*"; }
err() { printf 'jackdaw-install: error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Install jackdaw from a GitHub release for the current user.

Usage: install.sh [options]

  --version <v>       Install release v<v>, e.g. 0.19.0-rc.9 (default: latest)
  --prefix <dir>      Where versions are kept (default: $XDG_DATA_HOME/jackdaw/install)
  --bin-dir <dir>     Where the jackdaw and jd commands go (default: ~/.local/bin)
  --no-modify-path    Do not add the bin directory to shell startup files
  -y, --yes           Do not ask for confirmation
  --uninstall         Remove what this script installed
  -h, --help          Show this help

Environment: JACKDAW_VERSION, JACKDAW_INSTALL_DIR, JACKDAW_BIN_DIR,
JACKDAW_NO_MODIFY_PATH=1.
EOF
}

[ -n "${HOME:-}" ] || err "HOME is not set"

version="${JACKDAW_VERSION:-}"
prefix="${JACKDAW_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/jackdaw/install}"
bin_dir="${JACKDAW_BIN_DIR:-$HOME/.local/bin}"
base="${JACKDAW_DOWNLOAD_BASE:-}"
modify_path=yes
case "${JACKDAW_NO_MODIFY_PATH:-}" in ''|0) ;; *) modify_path=no ;; esac
yes=no
action=install

need_value() { if [ $# -lt 2 ] || [ -z "$2" ]; then err "$1 needs a value"; fi; }
while [ $# -gt 0 ]; do
    case "$1" in
        --version) need_value "$@"; version="$2"; shift ;;
        --version=*) version="${1#*=}" ;;
        --prefix) need_value "$@"; prefix="$2"; shift ;;
        --prefix=*) prefix="${1#*=}" ;;
        --bin-dir) need_value "$@"; bin_dir="$2"; shift ;;
        --bin-dir=*) bin_dir="${1#*=}" ;;
        --base-url) need_value "$@"; base="$2"; shift ;;
        --base-url=*) base="${1#*=}" ;;
        --no-modify-path) modify_path=no ;;
        -y|--yes) yes=yes ;;
        --uninstall) action=uninstall ;;
        -h|--help) usage; exit 0 ;;
        *) err "unknown option '$1' (see --help)" ;;
    esac
    shift
done

absolute() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$(pwd)" "$1" ;; esac; }
prefix=$(absolute "${prefix%/}")
bin_dir=$(absolute "${bin_dir%/}")

os=$(uname -s)
arch=$(uname -m)
unsupported() {
    err "no prebuilt jackdaw for $os $arch. Install with 'cargo install jackdaw --locked' instead; see https://jbuehler23.github.io/jackdaw/getting-started/installation.html"
}
case "$os" in
    Linux)
        case "$arch" in x86_64|amd64) ;; *) unsupported ;; esac
        if (ldd --version 2>&1 || true) | grep -qi musl; then unsupported; fi
        glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc //p')
        if [ -n "$glibc" ] && [ "$(printf '%s\n2.31\n' "$glibc" | sort -t . -k 1,1n -k 2,2n | head -n 1)" != 2.31 ]; then
            err "the prebuilt jackdaw needs glibc 2.31 or newer and this system has $glibc. Install with 'cargo install jackdaw --locked' instead"
        fi
        triple=x86_64-unknown-linux-gnu
        ext=tar.zst
        ;;
    Darwin)
        if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
            arch=arm64
        fi
        case "$arch" in arm64|aarch64) ;; *) unsupported ;; esac
        triple=aarch64-apple-darwin
        ext=zip
        ;;
    MINGW*|MSYS*|CYGWIN*) err "on Windows, use install.ps1 from PowerShell" ;;
    *) unsupported ;;
esac

confirm() {
    [ "$yes" = yes ] && return 0
    [ -t 0 ] || return 0
    printf '%s [Y/n] ' "$1"
    read -r answer || answer=n
    case "$answer" in ''|y|Y|yes|YES) ;; *) say "cancelled"; exit 1 ;; esac
}

is_bundle() { [ -d "$1" ] && [ ! -L "$1" ] && [ -f "$1/jd" ] && [ -f "$1/sdk/manifest.txt" ]; }

# A launcher is ours when it is the link or the shim this script writes.
is_ours() {
    if [ -L "$1" ]; then
        [ "$(readlink "$1")" = "$prefix/current/$2" ]
    else
        [ -f "$1" ] && grep -qF "$MARKER" "$1"
    fi
}

rc_files() {
    printf '%s\n' "$HOME/.profile"
    for f in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.bash_login"; do
        [ -f "$f" ] && printf '%s\n' "$f"
    done
    if command -v zsh >/dev/null 2>&1 || [ -f "${ZDOTDIR:-$HOME}/.zshenv" ]; then
        printf '%s\n' "${ZDOTDIR:-$HOME}/.zshenv"
    fi
    return 0
}
fish_conf="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/jackdaw.fish"

remove_rc_lines() {
    rc_files | while IFS= read -r f; do
        if [ -f "$f" ] && grep -qF "$MARKER" "$f"; then
            grep -vF "$MARKER" "$f" > "$f.jackdaw-tmp" || true
            cat "$f.jackdaw-tmp" > "$f"
            rm -f "$f.jackdaw-tmp"
            say "removed the PATH line from $f"
        fi
    done
    if [ -f "$fish_conf" ] && grep -qF "$MARKER" "$fish_conf"; then
        rm -f "$fish_conf"
        say "removed $fish_conf"
    fi
}

if [ "$action" = uninstall ]; then
    confirm "Remove jackdaw from $prefix?"
    for name in jackdaw jd; do
        if is_ours "$bin_dir/$name" "$name"; then rm -f "$bin_dir/$name"; fi
    done
    if [ -d "$prefix" ]; then
        for d in "$prefix"/*; do
            if is_bundle "$d"; then rm -rf "$d"; fi
        done
        for link in current previous; do
            if [ -L "$prefix/$link" ]; then rm -f "$prefix/$link"; fi
        done
        rmdir "$prefix" 2>/dev/null || say "left $prefix in place: it holds files this script did not install"
    fi
    remove_rc_lines
    say "jackdaw is uninstalled. Projects, settings, extensions and any SDK cache are kept."
    exit 0
fi

if [ -n "$base" ]; then
    secure=no
else
    secure=yes
fi
fetch() {
    if command -v curl >/dev/null 2>&1; then
        if [ "$secure" = yes ]; then
            curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
        else
            curl -fsSL --retry 3 -o "$2" "$1"
        fi
    elif command -v wget >/dev/null 2>&1; then
        if [ "$secure" = yes ]; then
            wget -q --https-only --secure-protocol=TLSv1_2 -O "$2" "$1"
        else
            wget -q -O "$2" "$1"
        fi
    else
        err "curl or wget is required"
    fi
}

# The latest release is read from where /releases/latest redirects, so the
# version directory is named before anything is downloaded and no API call
# (and no rate limit) is involved.
latest_version() {
    url="https://github.com/$REPO/releases/latest"
    if command -v curl >/dev/null 2>&1; then
        location=$(curl --proto '=https' --tlsv1.2 -fsSLI -o /dev/null -w '%{url_effective}' "$url") || location=""
    elif command -v wget >/dev/null 2>&1; then
        location=$(wget --https-only -S --spider --max-redirect=0 "$url" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r')
    else
        err "curl or wget is required"
    fi
    tag="${location##*/tag/}"
    if [ -z "$location" ] || [ "$tag" = "$location" ]; then
        err "could not find the latest release; pass --version"
    fi
    printf '%s' "$tag"
}

if [ -z "$version" ]; then
    [ -z "$base" ] || err "--base-url needs --version"
    version=$(latest_version)
fi
version="${version#v}"
case "$version" in ''|.*|*[!0-9A-Za-z.+-]*) err "'$version' is not a release version" ;; esac
[ -n "$base" ] || base="https://github.com/$REPO/releases/download/v$version"
base="${base%/}"

asset="jackdaw-$triple.$ext"
dest="$prefix/$version"
confirm "Install jackdaw $version to $dest?"

if [ "$ext" = tar.zst ]; then
    command -v tar >/dev/null 2>&1 || err "tar is required"
    command -v zstd >/dev/null 2>&1 || err "zstd is required to unpack $asset. Install it with your package manager (apt install zstd, dnf install zstd, pacman -S zstd) and run this again."
fi

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t jackdaw)
stage="$prefix/.staging.$$"
cleanup() { rm -rf "$tmp" "$stage"; }
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

say "downloading $base/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || err "could not download $asset.sha256 for $version"
fetch "$base/$asset" "$tmp/$asset" || err "could not download $asset for $version"

expected=$(tr -d '\r' < "$tmp/$asset.sha256" | sed -n 's/.*\([0-9a-fA-F]\{64\}\).*/\1/p' | head -n 1 | tr 'A-F' 'a-f')
[ -n "$expected" ] || err "$asset.sha256 holds no SHA-256 digest"
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)
else
    err "sha256sum or shasum is required to verify the download"
fi
[ "$actual" = "$expected" ] || err "checksum mismatch for $asset: expected $expected, got $actual. Nothing was installed."

mkdir -p "$prefix" "$bin_dir"
rm -rf "$stage"
mkdir "$stage"
case "$ext" in
    tar.zst)
        zstd -dcq "$tmp/$asset" | tar -xf - -C "$stage"
        ;;
    zip)
        if command -v ditto >/dev/null 2>&1; then
            ditto -x -k "$tmp/$asset" "$stage"
        elif command -v unzip >/dev/null 2>&1; then
            unzip -q "$tmp/$asset" -d "$stage"
        else
            err "ditto or unzip is required"
        fi
        ;;
esac
is_bundle "$stage/jackdaw-$triple" || err "$asset does not hold a jackdaw-$triple folder with jd and sdk/manifest.txt"
if [ "$os" = Darwin ]; then
    xattr -dr com.apple.quarantine "$stage/jackdaw-$triple" 2>/dev/null || true
fi

was_current=$(readlink "$prefix/current" 2>/dev/null || true)
if [ -e "$dest" ] || [ -L "$dest" ]; then
    mv "$dest" "$stage/replaced"
fi
mv "$stage/jackdaw-$triple" "$dest"
ln -sfn "$version" "$prefix/current"

# The version that was current before an upgrade is kept for rollback; older
# ones are removed.
if [ -n "$was_current" ] && [ "$was_current" != "$version" ]; then
    ln -sfn "$was_current" "$prefix/previous"
fi
previous=$(readlink "$prefix/previous" 2>/dev/null || true)
if [ "$previous" = "$version" ]; then
    rm -f "$prefix/previous"
    previous=""
fi
for d in "$prefix"/*; do
    name="${d##*/}"
    if [ "$name" != "$version" ] && [ "$name" != "$previous" ] && is_bundle "$d"; then
        rm -rf "$d"
    fi
done

for name in jackdaw jd; do
    link="$bin_dir/$name"
    if { [ -e "$link" ] || [ -L "$link" ]; } && ! is_ours "$link" "$name"; then
        say "replacing $link, which this script did not write"
    fi
    rm -f "$link"
    if [ "$os" = Darwin ]; then
        # macOS reports a program started through a symlink by the link's
        # path, and the editor finds its SDK next to that path, so the shim
        # starts the versioned binary by its real path, as Linux resolves it.
        quoted=$(printf '%s' "$dest/$name" | sed "s/'/'\\\\''/g")
        printf '#!/bin/sh\n%s\nexec '"'"'%s'"'"' "$@"\n' "$MARKER" "$quoted" > "$link"
        chmod 755 "$link"
    else
        ln -s "$prefix/current/$name" "$link"
    fi
done

say "installed jackdaw $version to $dest"

on_path() { case ":$PATH:" in *":$1:"*) return 0 ;; esac; return 1; }
case "$bin_dir" in
    "$HOME"/*) shown="\$HOME/${bin_dir#"$HOME"/}" ;;
    *) shown="$bin_dir" ;;
esac
if on_path "$bin_dir"; then
    found=$(command -v jackdaw 2>/dev/null || true)
    if [ -n "$found" ] && [ "$found" != "$bin_dir/jackdaw" ]; then
        say "warning: $found comes before $bin_dir on PATH and will run instead"
    fi
elif [ "$modify_path" = yes ]; then
    rc_files | while IFS= read -r f; do
        if [ -f "$f" ] && grep -qF "$MARKER" "$f"; then continue; fi
        # shellcheck disable=SC2016
        if printf '\nexport PATH="%s:$PATH" %s\n' "$shown" "$MARKER" 2>/dev/null >> "$f"; then
            say "added $bin_dir to PATH in $f"
        else
            say "could not write $f; add $bin_dir to PATH yourself"
        fi
    done
    if ! grep -qsF "$MARKER" "$fish_conf" && { command -v fish >/dev/null 2>&1 || [ -d "${fish_conf%/conf.d/*}" ]; }; then
        mkdir -p "${fish_conf%/*}"
        # shellcheck disable=SC2016
        printf 'contains -- "%s" $PATH; or set -gx PATH "%s" $PATH %s\n' "$shown" "$shown" "$MARKER" > "$fish_conf"
        say "added $bin_dir to PATH in $fish_conf"
    fi
    say "open a new terminal, or run: export PATH=\"$bin_dir:\$PATH\""
else
    say "$bin_dir is not on PATH; add it to run jackdaw by name"
fi

say "run 'jackdaw' to open the editor, or 'jd doctor' to check your setup"
