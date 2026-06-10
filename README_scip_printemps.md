# About `scip-printemps`

`scip-printemps` is a two-phase hybrid driver that runs
[SCIP](https://www.scipopt.org/) (linked in-process via the
[`russcip`](https://crates.io/crates/russcip) crate) for an initial budget and
then hands an incumbent solution plus the variables SCIP has proved to be
fixed (i.e. `lb == ub` at the root after presolve and root processing) over
to PRINTEMPS' bundled `pb_competition_2025_solver` for a heuristic
improvement phase.

It is the SCIP-flavoured counterpart of [`exact-printemps`](README_exact_printemps.md).
The orchestration is intentionally similar; the only differences are:

- Phase 1 is an in-process SCIP solve, not a subprocess invocation of Exact.
- The information funnelled to PRINTEMPS goes through a solver-agnostic
  `SolverHandoff` payload (`src/handoff.rs`) so future bidirectional or
  alternating compositions can plug in without changing call sites.
- The handoff carries the **dual bound** in addition to the incumbent and
  fixed-variable list. PRINTEMPS does not yet consume the dual bound; it is
  persisted to disk (`scip_bounds.json`) so it can be wired up later.

## Usage

```
scip-printemps [OPTIONS] <instance.opb>

  --scip-time SEC         Time budget for the SCIP phase (default: 300s).
  -t, --time-max SEC      Overall time budget; PRINTEMPS uses what's left.
  --printemps-path PATH   Path to pb_competition_2025_solver
                          (default: ./bin/pb_competition_2025_solver).
  --save-dir DIR          Directory for state files (default: ./.pb-scip-state).
  -r, --seed N            Random seed forwarded to both solvers.
  -j, --threads N         Number of threads forwarded to PRINTEMPS
                          (SCIP itself uses the core branch-and-bound
                          single-threaded; tune SCIP parallelism with
                          `--scip-arg parallel/maxnthreads=N` if your
                          build supports it).
  --scip-arg NAME=VALUE   Extra SCIP parameter (repeatable). The value is
                          interpreted as bool / int / real / string based
                          on its lexical form.
  --printemps-arg ARG     Extra argument to forward to PRINTEMPS (repeatable).
  --use-fixed-literals    Forward variables that SCIP has proved fixed
                          to PRINTEMPS via `-f` (default: disabled).
  --scip-max-intsize N    Skip the SCIP phase when the instance header's
                          intsize exceeds N (default: 53).
  --verbose               Enable driver-level logs on stderr.
  -h, --help              Show this help and exit.
```

The `PB_PRINTEMPS` environment variable overrides the default PRINTEMPS
binary path.

## Output behaviour

The driver writes PB-competition output (`c …`, `o …`, `v …`, `s …`) on
stdout. SCIP's own console messages are suppressed; a textual summary of the
solve is appended to `<save-dir>/scip_log.txt`.

- If SCIP returns `Optimal` / `Infeasible` (or `Satisfiable` on a decision
  instance), its verdict and incumbent are emitted as the final answer.
- Otherwise the SCIP-side `o …`, `s …`, and `v …` lines are emitted as
  comments (`c scip-objective: o …`, `c scip-final: s …`,
  `c scip-incumbent: v …`) and PRINTEMPS takes over.
- If PRINTEMPS finishes without a feasible solution while SCIP had one, the
  driver falls back to the SCIP incumbent and emits a final
  `s SATISFIABLE` / `v …` block based on it.

## Solution verification and large coefficients

SCIP solves in IEEE-754 `f64`, which represents integers exactly only up to
`2^53`. On instances whose coefficients are larger, SCIP can return an
incumbent that violates a constraint it believes satisfied — e.g. `a - b >= 1`
where `a, b ≈ 2^64` loses the `±1` and the constraint looks trivially
satisfiable. Two complementary guards protect against this.

- **Incumbent verification (always on).** Before SCIP's incumbent is used or
  persisted, the driver re-reads the original OPB and re-evaluates every
  constraint with exact `i128` arithmetic. If any constraint is violated — or
  the check cannot be completed exactly (a coefficient or running activity
  outside `i128` range, or an unparsable token) — the **entire** SCIP result is
  discarded: its incumbent, bounds, and verdict are dropped, the warm-start
  files are removed, and the driver falls through to PRINTEMPS as if SCIP had
  found nothing. The reason is recorded in `scip_log.txt`
  (`VERIFICATION REJECTED SCIP RESULT: …`) and `scip_bounds.json` records
  status `DISCARDED_VERIFICATION`. This catches *false-feasible* answers
  regardless of cause.

- **`intsize` skip (`--scip-max-intsize N`, default `53`).** PB-competition OPB
  files carry an `intsize=` field in their header comment (the bit length of
  the largest coefficient). When it exceeds `N`, the SCIP phase is skipped
  entirely and the instance is handed straight to PRINTEMPS
  (`c scip-printemps: skipping SCIP (intsize=… > …)`). The default `53` matches
  the f64 exact-integer limit. This additionally guards failures that
  verification *cannot* catch — a wrongly reported `UNSATISFIABLE` or a wrong
  optimum has no incumbent to re-check — and avoids spending the SCIP budget on
  instances it cannot handle reliably. Instances with no `intsize` header are
  never skipped on this basis; the incumbent verification above still applies.

Raise the threshold (e.g. `--scip-max-intsize 63`) to let SCIP attempt
larger-coefficient instances and rely on verification to reject any bad result;
set it very high to disable the skip altogether.

## Persisted state

Under `--save-dir` (default `./.pb-scip-state`):

- `scip_log.txt` — driver-level commentary on the SCIP run.
- `scip_incumbent.sol` — SCIP's best solution merged with the
  fixed-variable assignments, in the `xN VALUE` per-line format accepted by
  `pb_competition_2025_solver -i`.
- `scip_fixed_vars.txt` — only the variables SCIP proved to be fixed
  (`lb == ub` at the root), in the same format. When
  `--use-fixed-literals` is set, this file is also passed to PRINTEMPS
  via its `-f` option; otherwise it is written for auditing only.
- `scip_bounds.json` — `{ status, primal_bound, dual_bound, elapsed_sec,
  exit_code }`. `dual_bound` is captured but is not yet forwarded to
  PRINTEMPS. `status` is `DISCARDED_VERIFICATION` (with null bounds) when the
  incumbent failed exact verification; in that case `scip_incumbent.sol` and
  `scip_fixed_vars.txt` are removed so PRINTEMPS does not warm-start from a
  rejected solution.

The PRINTEMPS phase writes `printemps_log.txt` and `printemps_log.stderr.log`
exactly as in `exact-printemps`.

## Signal handling

`SIGINT`, `SIGTERM`, and `SIGXCPU` set an interrupt flag and, while
PRINTEMPS is running, are forwarded to it via `kill(pid, sig)` exactly as in
`exact-printemps`. Mid-SCIP interruption is **not** wired up yet: russcip's
`solve()` consumes the model so a side-thread cannot easily call
`SCIPinterruptSolve`. If a signal arrives during the SCIP phase the
interrupt flag is observed afterwards and PRINTEMPS is skipped; SCIP itself
will continue until its own time limit expires. This is a known gap that a
custom event handler could close in a future iteration.

## Extensibility

All inter-phase information lives in `pb_hybrid::handoff::SolverHandoff`:

```rust
pub struct SolverHandoff {
    pub incumbent: Option<Vec<VarAssignment>>,
    pub fixed_vars: Vec<VarAssignment>,
    pub primal_bound: Option<f64>,
    pub dual_bound: Option<f64>,
}
```

Adding a PRINTEMPS → SCIP direction is a matter of having
`pb_hybrid::printemps::run` return a `SolverHandoff` (e.g. by parsing its
final `v …` line) and passing it into `scip::run` as a *warm start*. The
SCIP stage can already consume such a payload to call `SCIPaddSol`. The
same design accommodates an alternating loop: drive `scip::run` and
`printemps::run` in turn, threading the handoff between them.

Wiring the dual bound into PRINTEMPS only requires adding a
`dual_bound: Option<f64>` field to `PrintempsConfig` and a matching CLI flag
on the bundled solver.

## Build

`russcip` is an **optional** dependency, gated on the `scip` Cargo feature.
Building only `exact-printemps` therefore does not pull in `russcip` /
`scip-sys` at all, and works on hosts without SCIP installed:

```sh
cargo build --release --bin exact-printemps
```

To build `scip-printemps` you must pick how SCIP is provided:

- **System SCIP** (`--features scip`): export `SCIPOPTDIR=/path/to/scip_install`
  (containing `lib/libscip.so` and `include/scip/`) and run
  `cargo build --release --bin scip-printemps --features scip`. The binary
  retains a runtime dependency on `libscip.so`.
- **Bundled SCIP** (`--features scip-bundled`): downloads a prebuilt SCIP
  shared library at build time. Convenient but the resulting binary still
  needs `libscip.so` at runtime (no static option available in this mode).
  Requires network access during the build.
- **From source** (`--features scip-from-source`, recommended for
  distributable binaries): downloads the scipoptsuite source and compiles
  SCIP with `-DSHARED=OFF`, producing a static `libscip.a`. The resulting
  `scip-printemps` only retains dynamic links to common system libraries
  (glibc, libstdc++, libgomp). Slower to build the first time; the
  scip-sys build artifacts are cached by `Swatinem/rust-cache` in CI.

### Recommended path: `./build.sh`

`BUILD_SCIP_PRINTEMPS=ON ./build.sh` builds both binaries in a single shot:

- `exact-printemps` is built first **without** the `scip` feature, so
  `russcip` / `scip-sys` never enter the dependency graph. With the default
  `STATIC=ON` this binary is fully statically linked (`+crt-static`).
- `scip-printemps` is then built with `--features $SCIP_PRINTEMPS_FEATURE`
  (default `scip-from-source`), which compiles SCIP from source with
  `-DSHARED=OFF`. The resulting binary links SCIP/SoPlex statically and
  keeps glibc/libstdc++/libgomp dynamic. `build.sh` asserts via `ldd` that
  neither `libscip` nor `libsoplex` is a dynamic dependency.

Override the SCIP mode by exporting `SCIP_PRINTEMPS_FEATURE=scip-bundled`
or `SCIP_PRINTEMPS_FEATURE=scip` (the latter requires a system SCIP at
`SCIPOPTDIR` and is the offline-friendly option).

At runtime, if SCIP is installed as a shared library somewhere non-standard,
set `LD_LIBRARY_PATH` so the loader can find `libscip.so`.
