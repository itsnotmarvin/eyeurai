#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: macos-smoke.sh /path/to/EyeUrAI.app" >&2
  exit 2
fi

app_path=$1
executable="$app_path/Contents/MacOS/eyeurai"
if [[ ! -x "$executable" ]]; then
  echo "EyeUrAI executable was not found at $executable" >&2
  exit 1
fi

smoke_dir=$(mktemp -d "${TMPDIR:-/tmp}/eyeurai-smoke.XXXXXX")
marker_path="$smoke_dir/native-bridge-ready.txt"
app_pid=""

cleanup() {
  if [[ -n "$app_pid" ]] && kill -0 "$app_pid" 2>/dev/null; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  rm -rf "$smoke_dir"
}
trap cleanup EXIT

"$executable" "--startup-smoke-marker=$marker_path" &
app_pid=$!

for _ in {1..80}; do
  if [[ -f "$marker_path" ]]; then
    marker=$(tr -d '\r\n' < "$marker_path")
    if [[ "$marker" != "native-bridge-ready" ]]; then
      echo "EyeUrAI wrote an invalid native bridge smoke marker: $marker" >&2
      exit 1
    fi
    echo "EyeUrAI packaged frontend reached the native bridge."
    exit 0
  fi
  if ! kill -0 "$app_pid" 2>/dev/null; then
    wait "$app_pid" || true
    echo "EyeUrAI exited before its native bridge became ready." >&2
    exit 1
  fi
  sleep 0.25
done

echo "EyeUrAI started, but its packaged frontend never reached the native command bridge." >&2
exit 1
