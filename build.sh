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
# When the `scip-from-source` feature is used (default), SCIP itself is linked
# statically into scip-printemps; common system libraries (glibc, libstdc++,
# libgcc_s, libgomp) remain dynamically linked because rust's `+crt-static`
# cannot cover SCIP's C++ runtime. exact-printemps therefore always has to be
# built in a separate cargo invocation when STATIC=ON.
BUILD_SCIP_PRINTEMPS="${BUILD_SCIP_PRINTEMPS:-OFF}"
SCIP_PRINTEMPS_FEATURE="${SCIP_PRINTEMPS_FEATURE:-scip-from-source}"

if [ "$STATIC" = "ON" ]; then
  echo "[pb] (1/2) exact-printemps with +crt-static"
  ( cd "$ROOT" && RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" \
      cargo build --release --bin exact-printemps )
  if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
    echo "[pb] (2/2) scip-printemps with --features $SCIP_PRINTEMPS_FEATURE (dynamic crt)"
    ( cd "$ROOT" && cargo build --release --bin scip-printemps \
        --features "$SCIP_PRINTEMPS_FEATURE" )
  fi
else
  if [ "$BUILD_SCIP_PRINTEMPS" = "ON" ]; then
    echo "[pb] exact-printemps + scip-printemps with --features $SCIP_PRINTEMPS_FEATURE (dynamic)"
    ( cd "$ROOT" && cargo build --release --bins \
        --features "$SCIP_PRINTEMPS_FEATURE" )
  else
    echo "[pb] exact-printemps (dynamic)"
    ( cd "$ROOT" && cargo build --release --bin exact-printemps )
  fi
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
