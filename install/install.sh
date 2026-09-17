#!/usr/bin/env sh
# Install ai-team: the `ait` binary, and the desktop app on macOS.
#
#   curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh
#
# Then:  ait doctor
set -eu

REPO="zottiben/ai-team"
REPO_URL="https://github.com/${REPO}"

say()  { printf '\033[1;36m==>\033[0m %s\n' "$1"; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$1"; }
warn() { printf '\033[33m!\033[0m %s\n' "$1" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

# A script in a clone can use its sibling files. A script piped to `sh` cannot: there
# `$0` is just "sh", and treating the current directory as its source tree can make an
# unrelated Cargo.toml win by accident.
here=""
# shellcheck disable=SC1007 # CDPATH is intentionally empty for this one command.
case "$0" in
  */*) here=$(CDPATH= cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd || true) ;;
esac

from_source=no
for arg in "$@"; do
  case "$arg" in
    --from-source) from_source=yes ;;
    -h|--help)
      echo "usage: install.sh [--from-source]"
      echo "  --from-source  build with cargo instead of downloading a release"
      exit 0 ;;
    *) die "unknown argument: $arg" ;;
  esac
done

# Pick a binary directory already on PATH, without sudo when possible.
if echo "$PATH" | tr ':' '\n' | grep -qx "$HOME/.local/bin"; then
  BIN_DIR="$HOME/.local/bin"
elif echo "$PATH" | tr ':' '\n' | grep -qx "$HOME/.cargo/bin"; then
  BIN_DIR="$HOME/.cargo/bin"
else
  BIN_DIR="/usr/local/bin"
fi

# --- prebuilt release -------------------------------------------------------------
#
# Preferred, because it needs no Rust toolchain and takes seconds. The frontend is
# compiled into the binary either way, so a downloaded `ait` has the full app.
install_release() {
  command -v curl >/dev/null 2>&1 || return 1
  command -v tar >/dev/null 2>&1 || return 1

  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)
  case "$os" in darwin|linux) ;; *) return 1 ;; esac

  version=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | grep '"tag_name"' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
  [ -n "$version" ] || return 1
  num="${version#v}"
  base="${REPO_URL}/releases/download/${version}"

  if [ "$os" = "darwin" ]; then
    file="ai-team-v${num}-macos-universal.tar.gz"
  else
    case "$arch" in
      x86_64|amd64)  larch=x86_64 ;;
      arm64|aarch64) larch=aarch64 ;;
      *) return 1 ;;
    esac
    file="ai-team-v${num}-linux-${larch}.tar.gz"
  fi

  tmp=$(mktemp -d) || return 1
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM

  say "Downloading ai-team ${version}"
  curl -fsSL "${base}/${file}" -o "${tmp}/${file}" || return 1

  # Best effort: only when checksums are published and a hasher exists.
  if curl -fsSL "${base}/checksums.txt" -o "${tmp}/checksums.txt" 2>/dev/null; then
    expected=$(grep " ${file}\$" "${tmp}/checksums.txt" | awk '{print $1}')
    if [ -n "$expected" ]; then
      if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "${tmp}/${file}" | awk '{print $1}')
      elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "${tmp}/${file}" | awk '{print $1}')
      else
        actual=""
      fi
      [ -z "$actual" ] || [ "$actual" = "$expected" ] \
        || die "checksum mismatch for ${file}"
    fi
  fi

  tar xzf "${tmp}/${file}" -C "$tmp" || return 1

  mkdir -p "$BIN_DIR" 2>/dev/null || true
  if [ -w "$BIN_DIR" ]; then
    install -m 0755 "${tmp}/ait" "${BIN_DIR}/ait"
  else
    sudo install -m 0755 "${tmp}/ait" "${BIN_DIR}/ait"
  fi
  ok "ait installed to ${BIN_DIR}/ait"
  INSTALLED_AIT="${BIN_DIR}/ait"

  # This marker deliberately outranks stale ~/.cargo install metadata. Without it,
  # replacing a cargo-installed binary with a release could make a later self-update
  # rebuild an old clone and silently downgrade the user.
  mkdir -p "$HOME/.ai-team"
  printf 'release\n' > "$HOME/.ai-team/install-method"

  # The desktop app, when the archive carries one. `ait ui` works regardless; this is
  # for people who would rather have it in the Dock.
  if [ -d "${tmp}/ai-team.app" ]; then
    rm -rf "/Applications/ai-team.app" 2>/dev/null || true
    if cp -R "${tmp}/ai-team.app" /Applications/ 2>/dev/null; then
      ok "ai-team.app installed to /Applications"
    else
      warn "could not write /Applications - run 'ait ui' in a browser instead"
    fi
  fi
  return 0
}

install_from_source() {
  command -v cargo >/dev/null 2>&1 \
    || die "no release for this platform and cargo is not installed - get Rust from https://rustup.rs"

  say "Building ait"
  if [ -n "$here" ] && [ -f "$here/Cargo.toml" ]; then
    cargo install --path "$here/crates/ai-team" --locked
  else
    # Behind a TLS-intercepting proxy, tell cargo to use the git CLI so it trusts the
    # system cert store:  export CARGO_NET_GIT_FETCH_WITH_CLI=true
    cargo install --git "$REPO_URL" ai-team --locked
  fi
  ok "ait installed"
  INSTALLED_AIT=$(command -v ait 2>/dev/null || printf '%s' "$HOME/.cargo/bin/ait")

  # A source install supersedes release provenance. Leaving the marker behind would
  # make a later self-update replace this build with a stock release.
  rm -f "$HOME/.ai-team/install-method"
}

if [ "$from_source" = yes ]; then
  install_from_source
elif install_release; then
  :
else
  # Expected until the first tag is pushed: there is no release to download yet.
  warn "no prebuilt release for this platform - building from source"
  install_from_source
fi

# Prove it runs before claiming success. An installer that reports "done" and leaves an
# unusable binary on PATH is worse than one that fails.
AIT=${INSTALLED_AIT:-$(command -v ait 2>/dev/null || printf '%s' "$HOME/.cargo/bin/ait")}
"$AIT" --version >/dev/null 2>&1 || die "$AIT was installed but will not run"
ok "$("$AIT" --version)"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on your PATH - add it to your shell profile" ;;
esac

cat <<'EOF'

Done. Next:
  ait doctor    # check the install: paths, machine profile, embedded frontend
  ait ui        # open the window in a browser
EOF
