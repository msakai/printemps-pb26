#!/usr/bin/env bash
# Build the Exact + PRINTEMPS + driver bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"
STATIC="${STATIC:-ON}"

echo "[pb] STATIC=$STATIC"

echo "[pb] building Exact"
mkdir -p "$ROOT/Exact/build"
( cd "$ROOT/Exact/build" \
  && cmake .. -DCMAKE_BUILD_TYPE=Release -Dbuild_static="$STATIC" \
  && cmake --build . -- -j"$JOBS" )

echo "[pb] building PRINTEMPS pb_competition_2025_solver"
( cd "$ROOT/printemps" && make -f makefile/Makefile.extra STATIC="$STATIC" -j"$JOBS" )

echo "[pb] building driver"
if [ "$STATIC" = "ON" ]; then
  ( cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo build --release )
else
  ( cd "$ROOT" && cargo build --release )
fi

mkdir -p "$ROOT/bin"
cp -f "$ROOT/Exact/build/Exact" "$ROOT/bin/"
cp -f "$ROOT/printemps/build/extra/Release/pb_competition_2025_solver" "$ROOT/bin/"
cp -f "$ROOT/target/release/pb-hybrid" "$ROOT/bin/"

echo "[pb] artifacts:"
ls -l "$ROOT/bin"
