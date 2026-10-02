#!/usr/bin/env bash
# PreToolUse (Write|Edit): block edits to generated files — regenerate from source.
# exit 2 blocks the call. Override patterns via GENERATED_GLOBS (pipe-separated regex).
set -uo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"; . "$DIR/_lib.sh"
# shellcheck disable=SC2034  # read by the helpers in _lib.sh, which shellcheck
# cannot follow through the runtime-computed $DIR above.
HOOK_JSON=$(cat)

json_have_parser || { echo "protect-generated: needs jq or python3 — guard INACTIVE, install one" >&2; exit 1; }

file=$(json_field tool_input file_path)
[ -z "$file" ] && exit 0

patterns="${GENERATED_GLOBS:-/generated/|/__generated__/|\.gen\.|\.pb\.go$|_pb2\.py$|\.pb\.ts$|\.g\.dart$|schema\.generated}"

echo "$file" | grep -qE "$patterns" \
  && { echo "BLOCKED: '$file' looks generated. Change the source and regenerate — never hand-edit generated output." >&2; exit 2; }
exit 0
