#!/bin/sh
# Offline planning seats. The Rust fixture performs scoped service writes while the
# fake planner is alive; real MCP transport is tested separately by plan_mcp.rs.
set -eu
[ -z "${ANTHROPIC_API_KEY:-}" ] || exit 93
[ "${PI_MCP_CONFIG_MODE:-}" = exclusive ] || exit 94
session=""
config=""
excluded=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) shift; session="$1" ;;
    --mcp-config) shift; config="$1" ;;
    --provider) shift; provider="$1" ;;
    --model) shift; model="$1" ;;
    --exclude-tools) shift; excluded="$1" ;;
    --) shift; prompt="$1"; break ;;
  esac
  shift
done
case "$prompt" in
  *"You are the orchestrator seat"*) role=orchestrator ;;
  *"You are the Pi coding agent"*) role=assistant ;;
  *) role=planner ;;
esac
if [ "$role" != assistant ]; then
  case "$excluded" in *write,edit*) ;; *) exit 95 ;; esac
fi
node=$(basename "$(dirname "$config")")
printf '%s' "$prompt" > "$TEAM_TEST_ROOT/$node.prompt"
echo "$$" > "$TEAM_TEST_ROOT/$node.group"
[ -n "$session" ] || session="session-$$"
printf '%s|%s|%s|%s|%s\n' "$role" "$config" "$session" "$provider" "$model" >> "$TEAM_TEST_ROOT/calls"
printf '{"type":"session","id":"%s","cwd":"%s"}\n' "$session" "$PWD"
printf '{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"CONTEXT_UNAVAILABLE: echoed prompt is not a verdict"}]}}\n'
mode=$(cat "$TEAM_TEST_ROOT/mode")
if [ "$mode" = "slow-$role" ]; then
  printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Still reading the proposed work"}}\n'
  trap 'exit 0' TERM
  sh -c 'trap "" TERM; echo $$ > "$TEAM_TEST_ROOT/tool.pid"; while :; do sleep 1; done' &
  printf '{"type":"tool_execution_start","toolName":"bash","args":{"command":"slow read"}}\n'
  while :; do sleep 1; done
fi
if [ "$role" = planner ] && [ "$mode" = normal ]; then
  while [ ! -f "$TEAM_TEST_ROOT/plan-ready" ]; do sleep 0.02; done
fi
text="Grounded brief; plan ready for human review."
if [ "$mode" = contextfail ]; then text="CONTEXT_UNAVAILABLE: required source denied"; fi
if [ "$mode" != empty ]; then
  printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"%s"}]}}\n' "$text"
fi
printf '{"type":"turn_end","message":{"role":"assistant","stopReason":"stop","usage":{"input":20,"output":10}}}\n'
printf '{"type":"agent_settled"}\n'
[ "$mode" != plannerexitfail ] || [ "$role" != planner ] || exit 7
