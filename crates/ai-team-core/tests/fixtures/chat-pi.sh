#!/bin/sh
# An offline Pi-shaped process for the conversation lifecycle integration test.
if [ -n "$ANTHROPIC_API_KEY" ]; then exit 93; fi
session=""
catalogue=""
config=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --list-models) catalogue=1 ;;
    --mcp-config) shift; config="$1" ;;
    --session-id) shift; session="$1" ;;
    --provider) shift; provider="$1" ;;
    --model) shift; model="$1" ;;
    --) shift; printf '%s\n' "$1" >> "$CHAT_TEST_ROOT/prompts"; break ;;
  esac
  shift
done
if [ -n "$catalogue" ]; then
  [ "$PI_MCP_CONFIG_MODE" = exclusive ] || exit 94
  [ -f "$config" ] && [ "$(cat "$config")" = '{"mcpServers":{}}' ] || exit 95
  printf '%s' "$config" > "$CHAT_TEST_ROOT/catalogue-config"
  printf 'provider model context max-out thinking images\nllama.cpp fixture-model 200K 4K yes no\n'
  exit 0
fi
[ -n "$session" ] || session="session-$$"
printf '%s|%s|%s|%s\n' "$session" "$PWD" "$provider" "$model" >> "$CHAT_TEST_ROOT/calls"
printf '{"type":"session","id":"%s","cwd":"%s"}\n' "$session" "$PWD"
printf '{"type":"turn_start"}\n'
printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Reading the working files…"}}\n'
printf 'kept\n' >> changed.txt
mode=$(cat "$CHAT_TEST_ROOT/mode")
if [ "$mode" = long-running ]; then
  i=0
  while [ "$i" -lt 64 ]; do
    printf '{"type":"turn_end","message":{"role":"assistant","content":[],"stopReason":"toolUse","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0}}}\n'
    printf '{"type":"turn_start"}\n'
    i=$((i + 1))
  done
  # Leave time for the supervisor to observe the counters before the agent settles.
  sleep 1
fi
if [ "$mode" = slow ]; then
  trap 'exit 0' TERM
  sh -c 'trap "" TERM; while :; do sleep 1; done' &
  echo "$!" > "$CHAT_TEST_ROOT/tool.pid"
  printf '{"type":"tool_execution_start","toolName":"bash","toolCallId":"slow-tool","args":{"command":"slow test"}}\n'
  while :; do sleep 1; done
fi
if [ "$mode" = orphan ] || [ "$mode" = inherited-pipes ]; then
  echo "$$" > "$CHAT_TEST_ROOT/orphan.group"
  if [ "$mode" = orphan ]; then
    sh -c 'trap "" TERM; echo $$ > "$CHAT_TEST_ROOT/orphan.pid"; while :; do sleep 1; done' >/dev/null 2>&1 &
  else
    sh -c 'trap "" TERM; echo $$ > "$CHAT_TEST_ROOT/orphan.pid"; while :; do sleep 1; done' &
  fi
  while [ ! -s "$CHAT_TEST_ROOT/orphan.pid" ]; do sleep 0.01; done
fi
if [ "$mode" = fail ]; then
  printf '{"type":"turn_end","message":{"role":"assistant","content":[],"stopReason":"error"}}\n'
else
  printf '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Files updated."}],"stopReason":"stop"}}\n'
  printf '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"Files updated."}],"stopReason":"stop","usage":{"input":100,"output":20,"cacheRead":0,"cacheWrite":0}}}\n'
fi
printf '{"type":"agent_settled"}\n'
[ "$mode" != exitfail ] || exit 7
