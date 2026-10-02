#!/usr/bin/env bash
# dotenv-header.sh — emit MCP auth headers as JSON, reading the token from a repo .env.
#
# For a REMOTE HTTP MCP whose token lives only in the repo's .env (never exported), a
# static "Authorization: Bearer ${VAR}" header can't work: Claude Code resolves ${VAR}
# from its OWN launch environment and *fails to parse the config* when it's unset. Use
# this as the server's `headersHelper` instead — Claude Code runs it at connection time
# and merges its JSON stdout into the request headers, so the token is read from .env
# then, not at launch. (with-dotenv.sh solves the same problem for stdio servers, by
# wrapping the process; an HTTP server has no local process to wrap.)
#
# Codex has no headersHelper. `ai-toolbox mcp` maps such a server to `bearer_token_env_var`
# in .codex/config.toml, which reads the token from the environment you launch codex from —
# so for Codex, export the token (it won't be read from the repo .env for a remote server).
#
# Pi has no headersHelper either, but a header value starting with `!` is run as a command
# at connect time and its stdout becomes the value — so `--raw` (below) covers Pi:
#   "headers": {"Authorization": "!.pi/mcp/dotenv-header.sh --raw EXAMPLE_API_TOKEN"}
#
# In .mcp.json (copy this to <repo>/.claude/mcp/dotenv-header.sh):
#   "type": "http",
#   "url": "https://api.example.com/mcp",
#   "headersHelper": ".claude/mcp/dotenv-header.sh EXAMPLE_API_TOKEN"
# emits:  {"Authorization":"Bearer <EXAMPLE_API_TOKEN from .env>"}
#
# Options (before the VAR name):
#   --env-file <path>   read a file other than ./.env
#   --header <name>     header name (default: Authorization)
#   --scheme <scheme>   prefix before the value (default: Bearer; pass "" for none)
#   --raw               print just the header VALUE, not a JSON headers object
set -euo pipefail

ENV_FILE=""; HEADER="Authorization"; SCHEME="Bearer"; RAW=0
while [ $# -gt 0 ]; do
  case "$1" in
    --env-file) ENV_FILE="${2:-}"; shift 2 ;;
    --header)   HEADER="${2:-}"; shift 2 ;;
    --scheme)   SCHEME="${2:-}"; shift 2 ;;
    --raw)      RAW=1; shift ;;
    --)         shift; break ;;
    -*)         echo "dotenv-header: unknown option $1" >&2; exit 2 ;;
    *)          break ;;
  esac
done

VAR="${1:-}"
[ -n "$VAR" ] || { echo "dotenv-header: usage: dotenv-header.sh [--header H] [--scheme S] VAR_NAME" >&2; exit 2; }

# headersHelper runs with cwd = the session's working directory (repo root); CLAUDE_PROJECT_DIR
# may be unset in that environment, so fall back to ./.env.
[ -n "$ENV_FILE" ] || ENV_FILE="${CLAUDE_PROJECT_DIR:-.}/.env"
# shellcheck source=/dev/null  # the .env path is chosen at run time - that is the point
[ -f "$ENV_FILE" ] && { set -a; . "$ENV_FILE"; set +a; }

VAL="${!VAR:-}"
[ -n "$VAL" ] || { echo "dotenv-header: '$VAR' is empty (not found in $ENV_FILE)" >&2; exit 1; }
[ -n "$SCHEME" ] && VAL="$SCHEME $VAL"

# Pi wants the bare value (it owns the header name); Claude Code wants a headers object.
if [ "$RAW" -eq 1 ]; then
  printf '%s\n' "$VAL"
  exit 0
fi

# JSON-escape backslash and double-quote, then emit a single-object headers map on stdout.
esc=${VAL//\\/\\\\}; esc=${esc//\"/\\\"}
printf '{"%s":"%s"}\n' "$HEADER" "$esc"
