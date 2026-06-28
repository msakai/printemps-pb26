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

echo "[pb] building driver binaries"
# scip-printemps is optional. Set BUILD_SCIP_PRINTEMPS=ON to also build it.
BUILD_SCIP_PRINTEMPS="${BUILD_SCIP_PRINTEMPS:-OFF}"
SCIP_PRINTEMPS_FEATURE="${SCIP_PRINTEMPS_FEATURE:-scip-from-source}"

if [ "$STATIC" = "ON" ]; then
  RUST_STATIC_FLAGS="-C target-feature=+crt-static"
else
  RUST_STATIC_FLAGS=""
fi

if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
  echo "[pb] exact-printemps + scip-printemps with --features $SCIP_PRINTEMPS_FEATURE"
  ( cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} ${RUST_STATIC_FLAGS}" \
      cargo build --release --bins --features "$SCIP_PRINTEMPS_FEATURE" --target x86_64-unknown-linux-gnu )
else
  echo "[pb] exact-printemps"
  ( cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} ${RUST_STATIC_FLAGS}" \
      cargo build --release --bin exact-printemps --target x86_64-unknown-linux-gnu )
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

if command -v ldd > /dev/null 2>&1 && [ "$BUILD_SCIP_PRINTEMPS" = "ON" ] && [ "$SCIP_PRINTEMPS_FEATURE" = "scip-from-source" ]; then
  echo "[pb] checking scip-printemps does not dynamically link SCIP/SoPlex"
  if ldd "$ROOT/bin/scip-printemps" | grep -E 'libscip|libsoplex'; then
    echo "ERROR: SCIP/SoPlex should be statically linked but appear in ldd output" >&2
    exit 1
  fi
fi

if command -v objdump > /dev/null 2>&1 && [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
  echo "[pb] checking glibc requirement of scip-printemps"
  objdump -T "$ROOT/bin/scip-printemps" | grep GLIBC | grep -oP 'GLIBC_[\d.]+' | sort -V | tail -1
  echo "[pb] checking libstdc++ requirement of scip-printemps"
  objdump -T "$ROOT/bin/scip-printemps" | grep GLIBCXX | grep -oP 'GLIBCXX_[\d.]+' | sort -V | tail -1
  objdump -T "$ROOT/bin/scip-printemps" | grep CXXABI | grep -oP 'CXXABI_[\d.]+' | sort -V | tail -1
fi
