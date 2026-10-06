#!/bin/sh
# Install or remove the witdiff binary.
#
#   ./install.sh                 # install the latest, into ~/.local/bin
#   ./install.sh --version v1.0.0-alpha.1
#   ./install.sh --to /usr/local/bin
#   ./install.sh --uninstall
#
# Downloads the archive for this platform from GitHub Releases, verifies it
# against SHA256SUMS, and moves the binary into place. Nothing is installed
# without the checksum passing.
#
# It never uses sudo. A destination that is not writable is reported with the
# command to run, rather than silently escalated.

set -eu

REPO="TahsinArafat/WitDiff"
# The canonical location of this script, so a hint can name a command the
# reader can actually run — including when they installed via `curl | sh` and
# have no local copy.
INSTALL_URL="https://raw.githubusercontent.com/${REPO}/main/install.sh"
INTEGRATIONS_URL="https://raw.githubusercontent.com/${REPO}/main/integrations/install.sh"
VERSION=""
DEST=""
UNINSTALL=0
FORCE=0

usage() {
  cat <<'EOF'
Install or remove the witdiff binary.

Usage: install.sh [options]

  --version <tag>   Release tag to install (default: the latest release).
  --to <dir>        Install directory (default: $HOME/.local/bin).
  --uninstall       Remove an installed witdiff and exit.
  --force           Install even if a different version is already present.
  -h, --help        Show this message.

The archive is verified against the release's SHA256SUMS before it is used.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --to)      DEST="${2:-}"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    --force)   FORCE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; usage >&2; exit 1 ;;
  esac
done

# --- platform detection -----------------------------------------------------
# The release publishes one archive per platform, named by target triple. A
# platform with no archive is reported rather than guessed at, because
# downloading the wrong architecture succeeds and then fails to execute.
detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Linux)
      case "$arch" in
        x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) return 1 ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        arm64|aarch64) echo "aarch64-apple-darwin" ;;
        *) return 1 ;;  # Intel macOS is not built; see docs/development.md
      esac
      ;;
    # Git Bash, MSYS2 and Cygwin report these, and they are the only way this
    # script runs on Windows: it is a POSIX shell script, so native PowerShell
    # and cmd users use the zip and the README's PowerShell block instead.
    # Without this branch the installer identified Windows and then refused it,
    # despite the release publishing a Windows archive.
    MINGW*|MSYS*|CYGWIN*)
      case "$arch" in
        x86_64|amd64) echo "x86_64-pc-windows-msvc" ;;
        *) return 1 ;;
      esac
      ;;
    *) return 1 ;;
  esac
}

# --- verify -----------------------------------------------------------------
# `shasum` is macOS, `sha256sum` is Linux, and neither is guaranteed. The
# fallback is openssl, which is present on both. Failing to verify is a refusal,
# never a warning: this step is the only thing standing between the user and an
# arbitrary download.
verify() {
  # The full path is hashed and only the basename is looked up, because the
  # archive lives in a temporary directory while SHA256SUMS names it plainly.
  # Passing the bare name to the hash tool worked only when the caller happened
  # to be in the same directory, which the installer is not.
  archive=$1
  sums=$2
  name=$(basename -- "$archive")
  expected=$(awk -v name="$name" '$2 == name { print $1 }' "$sums")
  [ -n "$expected" ] || { echo "error: $name is not listed in SHA256SUMS" >&2; return 1; }

  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$archive" | awk '{ print $1 }')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$archive" | awk '{ print $1 }')
  elif command -v openssl >/dev/null 2>&1; then
    actual=$(openssl dgst -sha256 "$archive" | awk '{ print $NF }')
  else
    echo "error: no sha256 tool found (looked for sha256sum, shasum, openssl)" >&2
    return 1
  fi

  if [ "$expected" != "$actual" ]; then
    echo "error: checksum mismatch for $archive" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    echo "The download is corrupt or has been tampered with. Not installing." >&2
    return 1
  fi
  echo "checksum verified: $name"
}

# --- uninstall --------------------------------------------------------------
installed_path() {
  # Both the requested destination and the default are considered, so
  # `--uninstall` finds a binary installed by an earlier run with `--to`.
  if [ -n "$DEST" ] && [ -f "$DEST/witdiff" ]; then echo "$DEST/witdiff"; return; fi
  if [ -f "$HOME/.local/bin/witdiff" ]; then echo "$HOME/.local/bin/witdiff"; return; fi
  if [ -f "/usr/local/bin/witdiff" ]; then echo "/usr/local/bin/witdiff"; return; fi
  if command -v witdiff >/dev/null 2>&1; then command -v witdiff; return; fi
  echo ""
}

if [ "$UNINSTALL" -eq 1 ]; then
  path=$(installed_path)
  if [ -z "$path" ]; then
    echo "witdiff is not installed in any location this script knows about"
    echo "(checked \$HOME/.local/bin and /usr/local/bin)"
    exit 0
  fi
  # An explicit `--to` names the location, so it is known by definition. The
  # guard below used to reject it: `installed_path` returns `$DEST/witdiff`
  # when `--to` is given, and the `case` then refused that same path as
  # "outside the known install locations" — so an install made with the two
  # documented flags could not be undone with them, and the error told the user
  # to remove by hand a file this script had just created.
  #
  # The property worth keeping is that every removal targets a path the
  # installer created: either the user named it, or it is one of the two
  # conventional directories. The `case` still enforces that.
  case "$path" in
    "$HOME"/*|/usr/local/bin/*)
      rm -f "$path"
      echo "removed $path"
      ;;
    *)
      if [ -n "$DEST" ] && [ "$path" = "$DEST/witdiff" ]; then
        rm -f "$path"
        echo "removed $path"
      else
        echo "error: $path is outside the known install locations; remove it yourself" >&2
        exit 1
      fi
      ;;
  esac
  echo
  echo "The agent integrations are separate. To remove those:"
  echo "  curl -fsSL $INTEGRATIONS_URL | sh -s -- --uninstall"
  echo "  (from a checkout: ./integrations/install.sh --uninstall)"
  exit 0
fi

# --- install ----------------------------------------------------------------
target=$(detect_target) || {
  echo "error: no prebuilt binary for $(uname -s) $(uname -m)" >&2
  echo "Build from source instead: cargo install --git https://github.com/${REPO} witdiff" >&2
  exit 1
}
echo "platform: $target"

[ -n "$DEST" ] || DEST="$HOME/.local/bin"
mkdir -p "$DEST" || { echo "error: cannot create $DEST" >&2; exit 1; }
[ -w "$DEST" ] || {
  echo "error: $DEST is not writable." >&2
  echo "Run this script with a writable --to, or install with:" >&2
  echo "  sudo install -m 0755 <extracted>/witdiff $DEST/witdiff" >&2
  exit 1
}

if [ -f "$DEST/witdiff" ] && [ "$FORCE" -eq 0 ]; then
  current=$("$DEST/witdiff" --version 2>/dev/null || echo "unknown")
  echo "witdiff is already installed at $DEST/witdiff ($current)"
  echo "Re-run with --force to replace it."
  exit 0
fi

if [ -z "$VERSION" ]; then
  # Not /releases/latest: an alpha is a prerelease, and that endpoint skips
  # those. Asking the API for the newest release of any kind is correct for
  # both a prerelease and a full release.
  VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases" \
    | grep -m1 '"tag_name"' | sed 's/.*: *"//; s/".*//')
  [ -n "$VERSION" ] || {
    echo "error: could not determine the latest release; pass --version" >&2
    exit 1
  }
fi
echo "version:  $VERSION"

# Windows ships a zip, every other target a tarball. The triple is named
# explicitly rather than pattern-matched on "windows", so adding a target
# without deciding its archive format is a visible omission.
case "$target" in
  x86_64-pc-windows-msvc) archive="witdiff-${target}.zip" ;;
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|aarch64-apple-darwin)
    archive="witdiff-${target}.tar.gz"
    ;;
  *)
    echo "error: $target has no known archive format" >&2
    exit 1
    ;;
esac
base="https://github.com/${REPO}/releases/download/${VERSION}"

work=$(mktemp -d)
# The temporary directory holds a downloaded executable and must not outlive
# the script, including on the error paths.
trap 'rm -rf "$work"' EXIT INT TERM

echo "downloading $archive"
curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$archive" "$base/$archive" || {
  echo "error: could not download $base/$archive" >&2
  exit 1
}
curl -fsSL --proto '=https' --tlsv1.2 -o "$work/SHA256SUMS" "$base/SHA256SUMS" || {
  echo "error: could not download SHA256SUMS; refusing to install unverified" >&2
  exit 1
}

verify "$work/$archive" "$work/SHA256SUMS"

case "$archive" in
  *.zip)
    command -v unzip >/dev/null 2>&1 || { echo "error: unzip is required" >&2; exit 1; }
    (cd "$work" && unzip -q "$archive")
    binary="$work/witdiff.exe"
    ;;
  *)
    # `-C` sets the working directory, so the archive is named relative to it.
    (cd "$work" && tar xzf "$archive")
    binary="$work/witdiff"
    ;;
esac
[ -f "$binary" ] || { echo "error: the archive did not contain witdiff" >&2; exit 1; }
chmod +x "$binary"

# Written to the destination and renamed, so an interrupted copy cannot leave a
# half-written binary where a working one was.
cp "$binary" "$DEST/.witdiff.new"
mv "$DEST/.witdiff.new" "$DEST/witdiff"

echo
echo "installed: $DEST/witdiff ($("$DEST/witdiff" --version 2>/dev/null || echo unknown))"
case ":$PATH:" in
  *":$DEST:"*) ;;
  *)
    echo
    echo "$DEST is not on your PATH. Add it:"
    echo "  export PATH=\"$DEST:\$PATH\""
    echo "(put that in ~/.profile, ~/.bashrc or ~/.zshrc to make it permanent)"
    ;;
esac
echo
echo "Next:"
echo "  witdiff doctor                      # check the toolchain is usable"
echo "  curl -fsSL $INTEGRATIONS_URL | sh   # wire it into your coding agent"
echo
# `$0` is the interpreter (often `sh`) when this is piped into a shell, so it
# cannot be trusted to name the script. When there is no local copy — the
# documented `curl | sh` path — the hint must give a command that works anyway:
# telling the reader to run `install.sh --uninstall` when the file exists only
# on GitHub is advice they cannot follow. The URL form always resolves.
# The installed binary can remove itself, and it is the one thing the reader
# definitely has. A relative or URL script reference is offered second, because
# `install.sh --uninstall` requires a copy that `curl | sh` never saves: measured
# from a real shell, that command answered `zsh: command not found: install.sh`.
echo "To remove: witdiff uninstall"
if [ -f "$0" ] && [ "$0" != "sh" ]; then
  echo "  (this copy: $0 --uninstall)"
else
  echo "  (no local copy of this script: curl -fsSL $INSTALL_URL | sh -s -- --uninstall)"
fi
