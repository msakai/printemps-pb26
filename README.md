# PRINTEMPS for Pseudo Boolean Competition 2026

A hybrid solver for the [Pseudo-Boolean Competition 2026][pbcomp].
It runs [Exact][exact] up to a configurable budget and, if Exact does not
deliver a final answer (`s OPTIMUM FOUND`, `s UNSATISFIABLE`, or `s SATISFIABLE`
on a decision instance), hands the same instance over to PRINTEMPS' bundled
`pb_competition_2025_solver` for a heuristic improvement phase.

[pbcomp]: https://www.cril.univ-artois.fr/PB26/
[exact]: https://gitlab.com/nonfiction-software/exact

## Layout

| Path | Description |
|------|-------------|
| `src/`             | Rust driver source (`pb26-hybrid`). |
| `Exact/`           | Submodule: Exact (AGPL-3.0). |
| `printemps/`       | Submodule: PRINTEMPS (MIT). |
| `build.sh`         | Builds all three components and copies binaries to `bin/`. |
| `Cargo.toml`       | Cargo manifest for the driver. |
| `LICENSE`          | MIT license for the driver itself; bundled components keep their own licenses. |

## Build

```sh
git submodule update --init
./build.sh
```

After a successful build:

```
bin/
├── Exact                          # AGPLv3, sources under Exact/
├── pb_competition_2025_solver     # MIT, sources under printemps/
└── pb26-hybrid                    # MIT, sources under src/
```

## Usage

```
pb26-hybrid [OPTIONS] <instance.opb>

  --exact-time SEC        Time budget for the Exact phase (default: 300s).
  -t, --time-max SEC      Overall time budget; PRINTEMPS uses what's left.
  --exact-path PATH       Path to the Exact binary (default: ./Exact/build/Exact).
  --printemps-path PATH   Path to pb_competition_2025_solver
                          (default: ./printemps/build/extra/Release/pb_competition_2025_solver).
  --save-dir DIR          Directory for state files (default: ./.pb26-state).
  -r, --seed N            Random seed forwarded to both solvers.
  -j, --threads N         Number of threads forwarded to both solvers.
  --exact-arg ARG         Extra argument to forward to Exact (repeatable).
  --printemps-arg ARG     Extra argument to forward to PRINTEMPS (repeatable).
  --verbose               Enable driver-level logs on stderr.
  -h, --help              Show this help and exit.
```

Both `PB26_EXACT` and `PB26_PRINTEMPS` environment variables override the
default solver paths.

### Output behaviour

The driver tees each child solver's stdout to its own stdout in PB
competition format (`c …`, `o …`, `v …`, `s …`).  The Exact phase additionally
buffers the trailing `s …` and `v …` lines: if Exact's verdict is final, those
lines are emitted as the answer; otherwise they are commented out
(`c exact-final: s SATISFIABLE`, `c exact-incumbent: v x1 -x2 …`) and PRINTEMPS
takes over.  If PRINTEMPS finishes without a feasible solution while Exact had
one, the driver falls back to Exact's saved incumbent and emits a final
`s SATISFIABLE` / `v …` block based on it.

### Persisted state

The Exact phase writes the following under `--save-dir` (default
`./.pb26-state`):

- `exact_log.txt` — full Exact stdout.
- `exact_log.stderr.log` — full Exact stderr.
- `exact_incumbent_pb.txt` — the last `v …` line printed by Exact.
- `exact_incumbent.sol` — the same solution, formatted for PRINTEMPS' `-i`
  (one `xN VALUE` line per variable).  Reserved for future hand-off; the
  current PRINTEMPS solver does not yet read it.
- `exact_bounds.json` — `{ status, primal_bound, elapsed_sec, exit_code }`.

The PRINTEMPS phase writes `printemps_log.txt` and `printemps_log.stderr.log`.

### Signal handling

`SIGINT`, `SIGTERM`, and `SIGXCPU` are caught by the driver and forwarded to
the active child via `kill(pid, sig)`.  Children are placed in their own
process group so terminal Ctrl-C does not double-deliver.  Both Exact and
PRINTEMPS gracefully emit their best-known solution on these signals; the
driver waits for them to flush before exiting.

## Licensing

The driver source under `src/`, the build script, and the Cargo manifest are
distributed under the MIT license (see `LICENSE`).

This bundle keeps the original licenses of its submodules unchanged:

- **Exact** (`Exact/`) — GNU Affero General Public License v3.0
  (`Exact/LICENSE`, `Exact/used_licenses/`).  Because the driver invokes Exact
  as a separate process rather than linking it, the AGPL does not propagate to
  the driver.  Source code for Exact is shipped alongside the binary in
  accordance with the AGPL.
- **PRINTEMPS** (`printemps/`) — MIT (`printemps/LICENSE`).

Any binary distribution of this bundle should retain the `Exact/`,
`printemps/`, and top-level `LICENSE` files unmodified.
