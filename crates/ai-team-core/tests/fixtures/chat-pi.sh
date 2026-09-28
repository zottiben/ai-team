#!/bin/sh
# An offline Pi-shaped process for the conversation lifecycle integration test.
if [ -n "$ANTHROPIC_API_KEY" ]; then exit 93; fi
session=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) shift; session="$1" ;;
    --provider) shift; provider="$1" ;;
    --model) shift; model="$1" ;;
    --) shift; printf '%s\n' "$1" >> "$CHAT_TEST_ROOT/prompts"; break ;;
  esac
  shift
done
[ -n "$session" ] || session="session-$$"
printf '%s|%s|%s|%s\n' "$session" "$PWD" "$provider" "$model" >> "$CHAT_TEST_ROOT/calls"
printf '{"type":"session","id":"%s","cwd":"%s"}\n' "$session" "$PWD"
printf '{"type":"turn_start"}\n'
printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Reading the working files…"}}\n'
printf 'kept\n' >> changed.txt
mode=$(cat "$CHAT_TEST_ROOT/mode")
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
