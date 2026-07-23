# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

A submission bundle for the [Pseudo-Boolean Competition 2026](https://www.cril.univ-artois.fr/PB26/). It ships three solvers built from two git submodules plus a Rust driver:

- `pb_competition_2025_solver` — the PRINTEMPS local-search solver (built from the `printemps/` submodule).
- `exact-printemps` — Rust driver that runs Exact (from `Exact/` submodule) then hands off to PRINTEMPS.
- `scip-printemps` — Rust driver that runs SCIP in-process (via `russcip`) then hands off to PRINTEMPS.

Both driver binaries are produced from the same Rust crate (`pb-hybrid`) at the repo root.

## Build & CI

- `git submodule update --init` is required before the first build.
- `BUILD_SCIP_PRINTEMPS=ON ./build.sh` builds everything and stages the four binaries into `bin/`. Without that env var, `scip-printemps` is skipped (the Cargo `scip` feature is opt-in because `russcip`/`scip-sys` are heavy build deps).
- `build.sh` defaults to `STATIC=ON`, which builds `exact-printemps` separately with `+crt-static` and `scip-printemps` with `--features scip-from-source` (statically links SCIP/SoPlex but keeps glibc and libstdc++ — along with libgcc_s/libm — dynamic; SCIP itself is built with `TPI=none` so `libgomp` is not pulled in). It then `ldd`-asserts that `libscip`/`libsoplex` are not dynamic in `scip-printemps`.
- For Rust-only iteration, `cargo build --release --bin exact-printemps` does **not** require SCIP. `scip-printemps` requires one of `--features scip` (system SCIP via `SCIPOPTDIR`), `scip-bundled`, or `scip-from-source`.
- CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy --release --bins --features scip-bundled -- -D warnings`, the full `build.sh`, and three smoke tests against `Exact/test/instances/opb/opt/air03.opb`. Run those locally before pushing — clippy is `-D warnings`.

Per-module unit tests live alongside the source (`#[cfg(test)] mod tests` in `src/{opb,verify,handoff,signals,exact,printemps,scip}.rs`) and run in CI as `cargo test --release --features scip-bundled`. End-to-end verification is the smoke tests in CI plus running solvers on real OPB instances.

## Architecture: how a hybrid run flows

Both drivers follow the same two-phase shape; the differences are concentrated in phase 1.

1. **Parse CLI + scan the OPB instance.** `src/opb.rs` does a cheap scan to detect whether the instance has an objective (`min:` line); this drives `s OPTIMUM FOUND` vs `s SATISFIABLE` semantics later. The restricted OPB input format the scan relies on (variable naming, header hints, `min:`-only objective, etc.) is specified in [misc/OPBcompetition.md](misc/OPBcompetition.md).
2. **Install signal forwarder** (`src/signals.rs`). A dedicated thread listens for `SIGINT`/`SIGTERM`/`SIGXCPU`, sets an `InterruptFlag`, and forwards the signal to the currently registered child PID (`ChildSlot`). Children are placed in their own process group so terminal Ctrl-C does not double-deliver. **Mid-SCIP interruption is a known gap** — `russcip::solve()` consumes the model so we can't call `SCIPinterruptSolve` from another thread; signals during phase 1 of `scip-printemps` are only observed after SCIP finishes naturally.
3. **Phase 1: Exact (subprocess) or SCIP (in-process).**
   - `src/exact.rs` spawns Exact, tees its stdout (passing PB-format lines through, buffering the trailing `s …`/`v …`), and parses `c fixed <signed-int>` lines when `--use-fixed-literals` is set.
   - `src/scip.rs` builds the SCIP model from the OPB file via `russcip`, runs the solve, then extracts the incumbent and root-level fixed variables (`lb == ub` after presolve/root).
4. **SolverHandoff** (`src/handoff.rs`) is the solver-agnostic payload threaded between phases. It carries `incumbent`, `fixed_vars`, `primal_bound`, `dual_bound`. The hand-off knows how to write the merged initial-solution file (fixed vars win on conflict, used via PRINTEMPS' `-i`), the fixed-vars-only file (PRINTEMPS' `-f`, when `--use-fixed-literals` is set), and a `*_bounds.json` snapshot. Adding a PRINTEMPS → SCIP/Exact direction is meant to mean returning a `SolverHandoff` from `printemps::run` and feeding it as a warm-start — keep new fields optional so each phase can populate only what it produces.
5. **Phase 2: PRINTEMPS.** `src/printemps.rs` spawns `pb_competition_2025_solver` with the remaining time budget, the seed/threads, the initial-solution file, and optionally the fixed-literals file. It tees stdout so the final PB-format lines flow through.
6. **Final answer selection.** Phase 1's verdict takes precedence when it is final (`OPTIMUM FOUND`/`UNSATISFIABLE`, or `SATISFIABLE` on a decision instance). Otherwise phase 1's `s …`/`v …` are downgraded to comments (`c exact-final: …`, `c scip-incumbent: …`) and PRINTEMPS' verdict is used. If PRINTEMPS finishes without a feasible solution but phase 1 had one, the driver falls back to phase 1's incumbent and emits a synthesized `s SATISFIABLE`/`v …`.

`src/lib.rs` exposes these modules as the `pb_hybrid` library crate; the two binaries (`src/bin/exact_printemps.rs`, `src/bin/scip_printemps.rs`) are thin CLI wrappers + orchestration around them.

## Persisted state

Each driver writes everything under `--save-dir` (defaults: `.pb-state` for exact, `.pb-scip-state` for scip):

- `exact_log.txt` / driver-level `scip_log.txt`, plus `*.stderr.log` for subprocesses.
- `*_incumbent.sol` — PRINTEMPS `-i` input.
- `exact_fixed_literals.txt` / `scip_fixed_vars.txt` — PRINTEMPS `-f` input (only used when `--use-fixed-literals`).
- `*_bounds.json` — `{ status, primal_bound, [dual_bound], elapsed_sec, exit_code }`.
- `printemps_log.txt` / `printemps_log.stderr.log`.

These directories are gitignored only implicitly (they are committed only if you `git add` them — don't).

## Output protocol

stdout must speak PB competition format: `c …` (comment), `o …` (objective), `v …` (variable assignment), `s …` (status). Anything the drivers emit that isn't the final answer is prefixed with `c`. The drivers themselves never print raw `s`/`v` lines from phase 1 directly — those are always buffered and either promoted to the final answer or downgraded to a `c …` comment, to guarantee at most one final `s`/`v` block.

The authoritative reference for the competition's file formats (restricted OPB for PBS/PBO, WBO) and solver requirements is [misc/OPBcompetition.md](misc/OPBcompetition.md) (rendered from the official [OPBcompetition.pdf](https://www.cril.univ-artois.fr/PB24/OPBcompetition.pdf), "Restricted OPB Format in Use in the PB Competitions"). Consult it when touching parsing, the integer-size handling, or output behavior — notably the `intsize=` header hint and the requirement to print `s UNSUPPORTED` at parse time when the solver cannot handle an instance's integer sizes.

## Conventions worth knowing

- The `scip` Cargo feature gates not only the `scip-printemps` binary but also the `pb_hybrid::scip` module (`#[cfg(feature = "scip")]` in `lib.rs`). Don't make `src/scip.rs` a hard dependency of anything in the `exact-printemps` build path.
- New inter-phase information should go through `SolverHandoff`, not new ad-hoc arguments. Keep fields optional.
- Driver-level logs go to stderr behind `--verbose`; never let them leak to stdout, which is reserved for PB-format output.

## Building the SCIP feature on Claude Code on the Web (sandbox network note)

In the Claude Code on the Web sandbox, all outbound HTTPS is intercepted by Anthropic's egress gateway (TLS cert issuer `O=Anthropic; CN=Egress Gateway SDS Issuing CA`). `curl`, Node, and Python trust it because the gateway CA is installed in the system trust store (`/etc/ssl/certs/ca-certificates.crt`, also exported as `SSL_CERT_FILE` / `REQUESTS_CA_BUNDLE` / `NODE_EXTRA_CA_CERTS`).

However, the `scip-sys` build script (pulled in by `russcip` for the `scip-bundled` and `scip-from-source` features) downloads SCIP with **`ureq` 2.x**, which uses rustls + the bundled `webpki-roots` Mozilla root set and ignores the system CA store. It therefore rejects the gateway certificate with `tls connection init failed: invalid peer certificate: UnknownIssuer`, so `cargo build`/`clippy --features scip-bundled` (and the real `scip-from-source` build in `build.sh`) fail to download SCIP. This is a TLS-trust issue, not a network block — the GitHub release URLs are reachable (e.g. `curl` returns 200).

Workaround: pre-fetch the archive with `curl` (which trusts the gateway CA) into the build's `OUT_DIR`, using the same "skip download" marker the build script checks, then rebuild. `OUT_DIR` is chosen by Cargo, so run a first build (it fails at the download), then locate it:

```sh
find target -type d -path '*scip-sys-*/out'
```

For `scip-bundled` (skip marker: `OUT_DIR/scip_install`):

```sh
OUT=$(find target/release -type d -path '*scip-sys-*/out')
curl -fsSL -o /tmp/libscip.zip \
  https://github.com/scipopt/scipoptsuite-deploy/releases/download/v0.9.0/libscip-linux.zip
unzip -q -o /tmp/libscip.zip -d "$OUT"        # creates $OUT/scip_install
cargo build --release --features scip-bundled --bin scip-printemps
```

For `scip-from-source` (skip marker: `OUT_DIR/scipoptsuite-9.2.4`; the build then compiles SCIP from these sources):

```sh
curl -fsSL -o /tmp/scipsrc.zip \
  https://github.com/scipopt/scip-sys/releases/download/v0.1.9/scipoptsuite-9.2.4.zip
unzip -q -o /tmp/scipsrc.zip -d "$OUT"         # creates $OUT/scipoptsuite-9.2.4
```

`cargo clean` wipes `OUT_DIR` (and may change its hash), so repeat after a clean.

Running (not just building) the `scip-bundled` binary additionally requires `libgfortran.so.5` and `libquadmath.so.0` (`apt-get install -y libgfortran5 libquadmath0` — on Ubuntu 24.04 the `libgfortran5` package does not pull `libquadmath0` automatically) and `LD_LIBRARY_PATH=$OUT/scip_install/lib` (the bundled `libscip.so` is linked without an rpath). The static `scip-from-source` build used by `build.sh` avoids both runtime issues. A full hybrid run also needs the phase-2 PRINTEMPS binary (`./bin/pb_competition_2025_solver`) produced by `build.sh`.
