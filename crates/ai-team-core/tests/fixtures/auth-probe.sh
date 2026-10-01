#!/bin/sh
# Metadata only. Every provider CLI in this test resolves here, never to the operator.
tool=$(basename "$0")
mode=$(cat "$AUTH_FIXTURE/mode")
printf '%s %s\n' "$tool" "$*" >> "$AUTH_FIXTURE/calls"
if [ -n "$ANTHROPIC_API_KEY$OPENAI_API_KEY$CLAUDE_CODE_USE_BEDROCK" ]; then
  printf '%s\n' "$tool" >> "$AUTH_FIXTURE/inherited"
fi
if [ "$tool" = pi ]; then
  provider=""
  while [ "$#" -gt 0 ]; do
    case "$1" in --provider) shift; provider="$1" ;; esac
    shift
  done
  case "$mode" in
    pi-ready|orphan|loud)
      if [ "$mode" = orphan ]; then
        sleep 8 &
        echo "$!" > "$AUTH_FIXTURE/child"
      fi
      if [ "$mode" = loud ]; then
        # Both pipes exceed their kernel buffers. Metadata capture must remain bounded.
        dd if=/dev/zero bs=1024 count=256 2>/dev/null
        dd if=/dev/zero bs=1024 count=256 2>/dev/null >&2
      fi
      printf '{"provider":"%s","status":"ready","authType":"oauth"}\n' "$provider" ;;
    pi-api-key) printf '{"provider":"%s","status":"ready","authType":"api_key"}\n' "$provider" ;;
    pi-foreign) printf '{"provider":"anthropic","status":"ready","authType":"oauth"}\n' ;;
    pi-failed) printf '{"provider":"%s","status":"ready","authType":"oauth"}\n' "$provider"; exit 1 ;;
    pi-unknown-type) printf '{"provider":"%s","status":"ready"}\n' "$provider" ;;
    *) printf '{"provider":"%s","status":"not_ready","reason":"provider_not_found"}\n' "$provider"; exit 1 ;;
  esac
elif [ "$tool" = claude ]; then
  case "$mode" in
    key) printf '{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}\n' ;;
    unknown) printf '{"loggedIn":true}\n' ;;
    third-party) printf '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"bedrock"}\n' ;;
    token) printf '{"loggedIn":true,"authMethod":"oauth_token","apiProvider":"firstParty"}\n' ;;
    *) printf '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}\n' ;;
  esac
elif [ "$tool" = codex ]; then
  case "$mode" in
    key) echo 'Logged in using an API key - SECRET_DECOY' >&2 ;;
    unknown) echo 'Signed in somehow' >&2 ;;
    *) echo 'Logged in using ChatGPT' >&2 ;;
  esac
fi
