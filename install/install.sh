#!/usr/bin/env sh
# Install ai-team: the `ait` binary, and the desktop app on macOS.
#
#   curl -fsSL https://zottiben.github.io/ai-team/install.sh | sh
# AI_TEAM_APP_DIR selects an alternate macOS bundle destination (also used by tests).
#
# Then:  ait doctor
set -eu

REPO="zottiben/ai-team"
REPO_URL="https://github.com/${REPO}"
APP_DIR="${AI_TEAM_APP_DIR:-/Applications/ai-team.app}"

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
      echo "  --from-source  explicitly build only the CLI with cargo (not the desktop app)"
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

bin_command() {
  if [ -w "$BIN_DIR" ]; then "$@"; else sudo "$@"; fi
}

cleanup() {
  rm -rf "$tmp"
  [ -z "${app_stage:-}" ] || rm -rf "$app_stage"
  [ -z "${cli_stage:-}" ] || bin_command rm -f "$cli_stage"
  # A previous app is recovery evidence, not temporary download content.
}

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

  # The public redirect does not consume the unauthenticated API's tiny quota.
  latest=$(curl -fsSL -o /dev/null -w '%{url_effective}' "${REPO_URL}/releases/latest") || return 1
  case "$latest" in
    "${REPO_URL}/releases/tag/"*) version=${latest##*/} ;;
    *) warn "unexpected latest-release URL: $latest"; return 1 ;;
  esac
  printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || return 1
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

  tmp=$(mktemp -d "${TMPDIR:-/tmp}/ai-team-install.XXXXXX") || return 1
  trap cleanup EXIT
  trap 'exit 1' HUP INT TERM

  say "Downloading ai-team ${version}"
  curl -fsSL "${base}/${file}" -o "${tmp}/${file}" || return 1

  curl -fsSL "${base}/checksums.txt" -o "${tmp}/checksums.txt" || return 1
  expected=$(awk -v file="$file" '$2 == file {print $1}' "${tmp}/checksums.txt")
  [ -n "$expected" ] || die "no published checksum for $file"
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "${tmp}/${file}" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "${tmp}/${file}" | awk '{print $1}')
  else
    die "install sha256sum or shasum before installing a release"
  fi
  [ "$actual" = "$expected" ] || die "checksum mismatch for $file"

  tar xzf "${tmp}/${file}" -C "$tmp" || return 1
  [ "$("${tmp}/ait" --version)" = "ait $num" ] || die "the downloaded CLI is not $version"

  # Stage both programs before replacing either. An app copy failure must not erase
  # the working app or leave a newer CLI looking like a successful desktop update.
  if [ "$os" = darwin ]; then
    [ -d "${tmp}/ai-team.app" ] || die "the macOS release contains no desktop app"
    mkdir -p "$(dirname "$APP_DIR")" || return 1
    app_stage=$(mktemp -d "${APP_DIR}.install.XXXXXX") || return 1
    cp -R "${tmp}/ai-team.app" "$app_stage/ai-team.app" || return 1
  fi
  mkdir -p "$BIN_DIR" 2>/dev/null || bin_command mkdir -p "$BIN_DIR" || return 1
  [ ! -d "$BIN_DIR/ait" ] || die "$BIN_DIR/ait is a directory"
  cli_stage=$(bin_command mktemp "$BIN_DIR/.ait.install.XXXXXX") || return 1
  bin_command install -m 0755 "${tmp}/ait" "$cli_stage" || return 1

  app_backup=""
  if [ "$os" = darwin ]; then
    if [ -e "$APP_DIR" ]; then
      app_backup=$(mktemp -d "${APP_DIR}.backup.XXXXXX") || return 1
      mv "$APP_DIR" "$app_backup/ai-team.app" || return 1
    fi
    if ! mv "$app_stage/ai-team.app" "$APP_DIR"; then
      [ -z "$app_backup" ] || mv "$app_backup/ai-team.app" "$APP_DIR" \
        || die "restore the previous app from $app_backup/ai-team.app"
      return 1
    fi
  fi
  if ! bin_command mv -f "$cli_stage" "$BIN_DIR/ait"; then
    if [ "$os" = darwin ]; then
      rm -rf "$APP_DIR" || die "the CLI swap failed; previous app is in $app_backup"
      [ -z "$app_backup" ] || mv "$app_backup/ai-team.app" "$APP_DIR" \
        || die "restore the previous app from $app_backup/ai-team.app"
    fi
    return 1
  fi
  cli_stage=""
  INSTALLED_AIT="${BIN_DIR}/ait"
  ok "ait installed to $INSTALLED_AIT"
  if [ "$os" = darwin ]; then
    ok "ai-team.app installed to $APP_DIR"
    [ -z "$app_backup" ] || say "Previous app saved in $app_backup"
  fi

  # Release provenance outranks stale cargo metadata only after both programs land.
  mkdir -p "$HOME/.ai-team" || return 1
  printf 'release\n' > "$HOME/.ai-team/install-method" || return 1
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

  # A source install supersedes release provenance. Leaving a stale `release` marker
  # would make a later self-update replace this build with a stock release - so it is
  # overwritten rather than removed, because "built from source" is something `ait update`
  # can act on and an absent marker is not: it cannot tell a source build from a binary
  # somebody's package manager owns, and has to refuse both.
  mkdir -p "$HOME/.ai-team"
  printf 'source\n' > "$HOME/.ai-team/install-method"
}

if [ "$from_source" = yes ]; then
  install_from_source
elif install_release; then
  :
else
  die "release installation failed; no CLI-only source fallback was attempted. Retry when the release is reachable, or explicitly use --from-source for only the CLI."
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
