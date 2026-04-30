#!/usr/bin/env bash
# Build the Exact + PRINTEMPS + driver bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"

echo "[pb26] building Exact"
mkdir -p "$ROOT/Exact/build"
( cd "$ROOT/Exact/build" \
  && cmake .. -DCMAKE_BUILD_TYPE=Release \
  && cmake --build . -- -j"$JOBS" )

echo "[pb26] building PRINTEMPS pb_competition_2025_solver"
( cd "$ROOT/printemps" && make -f makefile/Makefile.extra -j"$JOBS" )

echo "[pb26] building driver"
( cd "$ROOT" && cargo build --release )

mkdir -p "$ROOT/bin"
cp -f "$ROOT/Exact/build/Exact" "$ROOT/bin/"
cp -f "$ROOT/printemps/build/extra/Release/pb_competition_2025_solver" "$ROOT/bin/"
cp -f "$ROOT/target/release/pb26-hybrid" "$ROOT/bin/"

echo "[pb26] artifacts:"
ls -l "$ROOT/bin"
