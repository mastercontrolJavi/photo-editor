#!/bin/bash
# A quick check that the built Mac app starts: open it on a generated test
# photo, wait, take a screenshot, and report whether it was still running.
# Used by the GitHub build; a failure here does not stop the build, because
# GitHub's Macs are virtual machines and may not be able to show a window.
set -uo pipefail
cd "$(dirname "$0")/../.."

BIN="dist/dmg/Photo Editor.app/Contents/MacOS/photo-editor"
mkdir -p dist/smoke
REPORT=dist/smoke/result.txt

cargo run --release -p pipeline --bin pe -- fixtures dist/smoke/fixtures >/dev/null
cargo run --release -p pipeline --bin pe -- probe dist/smoke/fixtures/linear-landscape.dng > dist/smoke/probe.txt

"$BIN" dist/smoke/fixtures/linear-landscape.dng > dist/smoke/app-output.txt 2>&1 &
PID=$!
sleep 15
if kill -0 "$PID" 2>/dev/null; then
  echo "RUNNING: the app was still open after 15 seconds" | tee "$REPORT"
  screencapture -x dist/smoke/mac-screenshot.png 2>>"$REPORT" || echo "No screenshot could be taken" | tee -a "$REPORT"
  kill "$PID" 2>/dev/null
  exit 0
else
  wait "$PID"
  echo "EXITED: the app closed by itself with code $?" | tee "$REPORT"
  tail -20 dist/smoke/app-output.txt | tee -a "$REPORT"
  exit 1
fi
