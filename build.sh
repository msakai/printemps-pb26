#!/usr/bin/env bash
# Build the Exact + PRINTEMPS + driver bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"
STATIC="${STATIC:-ON}"
CPU_ARCH="${CPU_ARCH:-native}"

echo "[pb] STATIC=$STATIC"

echo "[pb] building Exact"
mkdir -p "$ROOT/Exact/build"
( cd "$ROOT/Exact/build" \
  && cmake .. -DCMAKE_BUILD_TYPE=Release -Dbuild_static="$STATIC" \
  && cmake --build . -- -j"$JOBS" )

echo "[pb] building PRINTEMPS pb_competition_2025_solver"
( cd "$ROOT/printemps" && make -f makefile/Makefile.extra STATIC="$STATIC" CPU_ARCH="$CPU_ARCH" -j"$JOBS" )

echo "[pb] building exact-printemps"
# exact-printemps is built first, with russcip absent from the dependency
# graph (no `scip` feature), so SCIP is not required to be installed.
if [ "$STATIC" = "ON" ]; then
  ( cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" \
      cargo build --release --bin exact-printemps )
else
  ( cd "$ROOT" && cargo build --release --bin exact-printemps )
fi

# scip-printemps is optional. Set BUILD_SCIP_PRINTEMPS=ON to also build it.
# SCIP itself is linked statically (via the `scip-from-source` feature, which
# compiles SCIP with -DSHARED=OFF), but common system libraries (glibc,
# libstdc++, libgcc_s, libgomp) remain dynamically linked because rust's
# `+crt-static` cannot cover SCIP's C++ runtime.
BUILD_SCIP_PRINTEMPS="${BUILD_SCIP_PRINTEMPS:-OFF}"
SCIP_PRINTEMPS_FEATURE="${SCIP_PRINTEMPS_FEATURE:-scip-from-source}"

if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
  echo "[pb] building scip-printemps (--features $SCIP_PRINTEMPS_FEATURE)"
  ( cd "$ROOT" && cargo build --release --bin scip-printemps \
      --features "$SCIP_PRINTEMPS_FEATURE" )
fi

mkdir -p "$ROOT/bin"
cp -f "$ROOT/Exact/build/Exact" "$ROOT/bin/"
cp -f "$ROOT/printemps/build/extra/Release/pb_competition_2025_solver" "$ROOT/bin/"
cp -f "$ROOT/target/release/exact-printemps" "$ROOT/bin/"
if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
  cp -f "$ROOT/target/release/scip-printemps" "$ROOT/bin/"
fi

echo "[pb] artifacts:"
ls -l "$ROOT/bin"

echo "[pb] linkage:"
LINKAGE_TARGETS=("$ROOT/bin/Exact" "$ROOT/bin/pb_competition_2025_solver" "$ROOT/bin/exact-printemps")
if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
  LINKAGE_TARGETS+=("$ROOT/bin/scip-printemps")
fi
for f in "${LINKAGE_TARGETS[@]}"; do
  file "$f"
done
