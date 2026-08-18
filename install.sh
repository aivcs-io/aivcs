#!/bin/sh
# AIVCS installer.
#
#   curl -sSL https://www.aivcs.io/install.sh | sh
#
# Downloads a prebuilt release binary, verifies it against the release
# SHA256SUMS, and installs it. Refuses to install anything it could not
# checksum — a pipe-to-shell installer that skips verification is worse than no
# installer, because it looks trustworthy while being trivially tampered with.
#
# Environment:
#   AIVCS_VERSION   version to install, e.g. 0.4.3 (default: latest release)
#   AIVCS_BIN_DIR   install directory (default: ~/.local/bin, or /usr/local/bin
#                   when writable)
#   AIVCS_BASE_URL  override the artifact host (default: GitHub releases)

set -eu

REPO="aivcs-io/aivcs"
BIN="aivcs"

say() { printf '%s\n' "$*"; }
err() { printf 'install: %s\n' "$*" >&2; }
die() { err "$*"; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

# --- platform -------------------------------------------------------------

detect_platform() {
    os=$(uname -s)
    arch=$(uname -m)

    case "$os" in
        Linux)  os=linux ;;
        Darwin) os=darwin ;;
        *) die "unsupported operating system: $os
  Build from source instead:
    cargo install --git https://github.com/$REPO aivcs-cli" ;;
    esac

    case "$arch" in
        x86_64|amd64)  arch=x86_64 ;;
        arm64|aarch64) arch=arm64 ;;
        *) die "unsupported architecture: $arch" ;;
    esac

    if [ "$os" = linux ]; then
        # The Linux builds are dynamically linked against glibc and OpenSSL 3.
        # Both failures surface as an opaque loader error *after* a successful
        # install, which looks like a corrupt download rather than an
        # unsupported system. Check here instead.
        if ! ldd --version 2>&1 | grep -qi glibc; then
            die "musl-based system detected (Alpine and similar); the prebuilt Linux
  binaries are linked against glibc. Build from source instead:
    cargo install --git https://github.com/$REPO aivcs-cli"
        fi
        if ! have_libssl3; then
            die "OpenSSL 3 not found (need libssl.so.3 and libcrypto.so.3).
  This is present on Debian 12+, Ubuntu 22.04+, RHEL 9+ and similar.
  On an older distribution, install from source instead:
    cargo install --git https://github.com/$REPO aivcs-cli"
        fi
    fi

    PLATFORM="${os}-${arch}"
}

# Look for OpenSSL 3 without assuming ldconfig exists or that any particular
# multiarch directory layout is in use.
have_libssl3() {
    if command -v ldconfig >/dev/null 2>&1; then
        ldconfig -p 2>/dev/null | grep -q 'libssl\.so\.3' && return 0
    fi
    for d in /usr/lib /usr/lib64 /lib /lib64 \
             /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu; do
        [ -e "$d/libssl.so.3" ] && return 0
    done
    return 1
}

# --- version --------------------------------------------------------------

latest_version() {
    # Resolve without jq: the API is only consulted for the tag name.
    curl -sSL -H 'Accept: application/vnd.github+json' \
        "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' \
        | head -n 1
}

# --- install --------------------------------------------------------------

choose_bin_dir() {
    if [ -n "${AIVCS_BIN_DIR:-}" ]; then
        printf '%s' "$AIVCS_BIN_DIR"
    elif [ -w /usr/local/bin ] 2>/dev/null; then
        printf '%s' /usr/local/bin
    else
        printf '%s' "$HOME/.local/bin"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        die "need sha256sum or shasum to verify the download"
    fi
}

main() {
    need curl
    need uname

    detect_platform

    version="${AIVCS_VERSION:-}"
    if [ -z "$version" ]; then
        version=$(latest_version) || true
        [ -n "$version" ] || die "could not determine the latest version;
  set AIVCS_VERSION explicitly, e.g. AIVCS_VERSION=0.4.3"
    fi

    base="${AIVCS_BASE_URL:-https://github.com/$REPO/releases/download/v$version}"
    asset="${BIN}-${PLATFORM}"

    tmp=$(mktemp -d 2>/dev/null || mktemp -d -t aivcs)
    # shellcheck disable=SC2064
    trap "rm -rf '$tmp'" EXIT INT TERM

    say "aivcs $version ($PLATFORM)"

    if ! curl -sSLf -o "$tmp/$asset" "$base/$asset"; then
        die "no prebuilt binary for $PLATFORM at $base/$asset
  Available alternatives:
    brew install aivcs-io/tap/aivcs
    cargo install --git https://github.com/$REPO aivcs-cli"
    fi

    # Verification is mandatory. If SHA256SUMS is missing the release is
    # malformed, and installing anyway would defeat the point of publishing it.
    if ! curl -sSLf -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"; then
        die "release $version has no SHA256SUMS; refusing to install unverified"
    fi

    want=$(grep -E "[[:space:]]\*?${asset}\$" "$tmp/SHA256SUMS" | cut -d' ' -f1 | head -n 1)
    [ -n "$want" ] || die "no checksum for $asset in SHA256SUMS; refusing to install"

    got=$(sha256_of "$tmp/$asset")
    if [ "$want" != "$got" ]; then
        die "checksum mismatch for $asset
  expected $want
  actual   $got
  Not installing. Re-run; if it persists, report it."
    fi

    dir=$(choose_bin_dir)
    mkdir -p "$dir" || die "cannot create $dir"
    chmod 0755 "$tmp/$asset"
    mv "$tmp/$asset" "$dir/$BIN" || die "cannot install into $dir"

    say "installed $dir/$BIN"

    case ":${PATH}:" in
        *":$dir:"*) ;;
        *) say ""
           say "note: $dir is not on your PATH. Add it:"
           say "  export PATH=\"$dir:\$PATH\"" ;;
    esac

    if [ -x "$dir/$BIN" ]; then
        "$dir/$BIN" --version || true
    fi
}

main "$@"
