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

echo "[pb] building driver"
# scip-printemps depends on `russcip`, which links against SCIP. By default we
# build only the exact-printemps binary so the bundle stays buildable on hosts
# without SCIP. Set BUILD_SCIP_PRINTEMPS=ON (and provide SCIP via SCIPOPTDIR,
# a system install, or the `bundled-scip` cargo feature) to also build
# scip-printemps.
BUILD_SCIP_PRINTEMPS="${BUILD_SCIP_PRINTEMPS:-OFF}"
CARGO_FEATURES="${CARGO_FEATURES:-}"

cargo_build() {
  local bins=("--bin" "exact-printemps")
  if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
    bins+=("--bin" "scip-printemps")
  fi
  local feat_args=()
  if [ -n "$CARGO_FEATURES" ]; then
    feat_args=("--features" "$CARGO_FEATURES")
  fi
  ( cd "$ROOT" && cargo build --release "${bins[@]}" "${feat_args[@]}" )
}

if [ "$STATIC" = "ON" ] && [ "$BUILD_SCIP_PRINTEMPS" != "ON" ]; then
  ( cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" cargo build --release --bin exact-printemps )
else
  # SCIP pulls in libgfortran/libgmp/etc., so +crt-static cannot produce a
  # fully static scip-printemps without a custom toolchain. Fall back to the
  # default linkage when the SCIP binary is part of the build set.
  cargo_build
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
