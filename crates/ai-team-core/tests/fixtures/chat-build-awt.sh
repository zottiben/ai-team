#!/bin/sh
set -eu
case "$1" in
  get)
    printf '%s\n' "$*" >> "$BUILD_TEST_ROOT/awt-calls"
    count=$(wc -l < "$BUILD_TEST_ROOT/awt-calls" | tr -d ' ')
    mode=$(cat "$BUILD_TEST_ROOT/awt-mode")
    if [ "$mode" = root ]; then pwd; exit 0; fi
    if [ "$mode" = foreign ]; then printf '%s\n' "$BUILD_TEST_ROOT/foreign"; exit 0; fi
    path="$BUILD_TEST_ROOT/lease-$count"
    git worktree add --detach "$path" HEAD >&2
    if [ "$mode" = dirty ]; then printf 'keep this file\n' > "$path/keep.txt"; fi
    if [ "$mode" = changed-source ]; then printf 'new human draft\n' > later.txt; fi
    if [ "$mode" = pause ]; then
      touch "$BUILD_TEST_ROOT/awt-waiting"
      count=0
      while [ ! -e "$BUILD_TEST_ROOT/awt-release" ] && [ "$count" -lt 1000 ]; do
        sleep 0.01
        count=$((count + 1))
      done
      [ -e "$BUILD_TEST_ROOT/awt-release" ] || exit 88
    fi
    if [ "$mode" = fail ]; then exit 19; fi
    printf '%s\n' "$path"
    ;;
  return)
    touch "$BUILD_TEST_ROOT/awt-returned"
    echo 'preparation must never silently destroy a lease' >&2
    exit 90
    ;;
  *) echo 'unexpected awt operation' >&2; exit 91 ;;
esac
