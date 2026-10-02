#!/usr/bin/env bash
# with-dotenv.sh — load a project .env into the environment, then exec a command.
# Lets MCP servers (and any CLI) read secrets from a root .env WITHOUT you having
# to `source` it in your shell first, and supports multiple environments.
#
# Cross-harness: Claude Code expands ${VAR} in .mcp.json before launch, so the args
# arrive already-resolved. Codex does NOT interpolate config strings — so after
# loading .env this script also expands any ${VAR} left in the trailing args (a
# no-op for Claude, since nothing remains). That makes it the one bridge that lets
# a ${VAR}-in-args server work under both harnesses. Codex path: .codex/mcp/with-dotenv.sh.
#
# In .mcp.json (copy this script to <repo>/.claude/mcp/with-dotenv.sh):
#   "command": ".claude/mcp/with-dotenv.sh",
#   "args": ["--", "npx", "-y", "@supabase/mcp-server-supabase@latest", "--read-only", "--project-ref=<ref>"]
#
# --need VAR (repeatable): document + require an env var. Fails with a clear
#   message if it's empty after loading .env, instead of letting the server start
#   and error later. Makes the config self-documenting about the secret it uses:
#   "args": ["--need", "SUPABASE_ACCESS_TOKEN", "--", "npx", …]
#
# --set TARGET=SOURCE (repeatable): remap a differently-named var onto the name the
#   server expects (e.g. a per-env token):
#   "args": ["--set", "SUPABASE_ACCESS_TOKEN=SUPABASE_STAGING_TOKEN", "--", "npx", …]
#
# --env-file <path>: use a file other than ./.env (e.g. .env.staging).
# Assumes a simple KEY=value .env (the shell sources it).
set -euo pipefail

ENV_FILE=""
REMAPS=()
NEEDS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --env-file) ENV_FILE="${2:-}"; shift 2 ;;
    --set)      REMAPS+=("${2:-}"); shift 2 ;;
    --need)     NEEDS+=("${2:-}"); shift 2 ;;
    --)         shift; break ;;
    *)          break ;;
  esac
done
[ -n "$ENV_FILE" ] || ENV_FILE="${CLAUDE_PROJECT_DIR:-.}/.env"

if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck source=/dev/null  # the .env path is chosen at run time - that is the point
  . "$ENV_FILE"
  set +a
fi

if [ "${#REMAPS[@]}" -gt 0 ]; then
  for m in "${REMAPS[@]}"; do
    tgt="${m%%=*}"; src="${m#*=}"
    export "$tgt"="${!src:-}"
  done
fi

if [ "${#NEEDS[@]}" -gt 0 ]; then
  for v in "${NEEDS[@]}"; do
    if [ -z "${!v:-}" ]; then
      echo "with-dotenv: required env var '$v' is empty (not found in $ENV_FILE)" >&2
      exit 1
    fi
  done
fi

# Expand ${VAR} references left in the args, using the now-loaded environment. Claude
# Code already resolved these before launch (so this is a no-op there); Codex passes
# config strings through verbatim, so this is what makes ${VAR}-in-args work under Codex.
expand_vars() {
  local s=$1 out='' name val
  while [[ $s == *'${'*'}'* ]]; do
    out+=${s%%'${'*}          # text before the first ${
    s=${s#*'${'}              # drop up to and including the first ${
    name=${s%%'}'*}           # var name, up to the first }
    s=${s#*'}'}               # remainder after the first }
    if [[ $name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      val=${!name-}; out+=$val
    else
      out+='${'"$name"'}'     # not a valid var name — leave it literal
    fi
  done
  out+=$s
  printf '%s' "$out"
}
if [ "$#" -gt 0 ]; then
  args=()
  for a in "$@"; do args+=("$(expand_vars "$a")"); done
  set -- "${args[@]}"
fi

exec "$@"
