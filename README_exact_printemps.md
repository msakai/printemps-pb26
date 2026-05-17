# About `exact-printemps`

It runs [Exact](https://gitlab.com/nonfiction-software/exact) up to a configurable budget and,
if Exact does not deliver a final answer (`s OPTIMUM FOUND`, `s UNSATISFIABLE`, or `s SATISFIABLE`
on a decision instance), hands the same instance over to PRINTEMPS' bundled `pb_competition_2025_solver`
for a heuristic improvement phase.

## Usage

```
exact-printemps [OPTIONS] <instance.opb>

  --exact-time SEC        Time budget for the Exact phase (default: 300s).
  -t, --time-max SEC      Overall time budget; PRINTEMPS uses what's left.
  --exact-path PATH       Path to the Exact binary (default: ./bin/Exact).
  --printemps-path PATH   Path to pb_competition_2025_solver
                          (default: ./bin/pb_competition_2025_solver).
  --save-dir DIR          Directory for state files (default: ./.pb-state).
  -r, --seed N            Random seed forwarded to both solvers.
  -j, --threads N         Number of threads forwarded to both solvers.
  --exact-arg ARG         Extra argument to forward to Exact (repeatable).
  --printemps-arg ARG     Extra argument to forward to PRINTEMPS (repeatable).
  --use-fixed-literals    Read ` c fixed <signed-int>` lines from Exact's
                          output and forward them to PRINTEMPS via `-f`
                          (default: disabled).
  --verbose               Enable driver-level logs on stderr.
  -h, --help              Show this help and exit.
```

Both `PB_EXACT` and `PB_PRINTEMPS` environment variables override the
default solver paths.

## Output behaviour

The driver tees each child solver's stdout to its own stdout in PB
competition format (`c …`, `o …`, `v …`, `s …`).  The Exact phase additionally
buffers the trailing `s …` and `v …` lines: if Exact's verdict is final, those
lines are emitted as the answer; otherwise they are commented out
(`c exact-final: s SATISFIABLE`, `c exact-incumbent: v x1 -x2 …`) and PRINTEMPS
takes over.  If PRINTEMPS finishes without a feasible solution while Exact had
one, the driver falls back to Exact's saved incumbent and emits a final
`s SATISFIABLE` / `v …` block based on it.

## Persisted state

The Exact phase writes the following under `--save-dir` (default
`./.pb-state`):

- `exact_log.txt` — full Exact stdout.
- `exact_log.stderr.log` — full Exact stderr.
- `exact_incumbent_pb.txt` — the last `v …` line printed by Exact.
- `exact_incumbent.sol` — the same solution, formatted for PRINTEMPS' `-i`
  (one `xN VALUE` line per variable).
- `exact_bounds.json` — `{ status, primal_bound, elapsed_sec, exit_code }`.
- `exact_fixed_literals.txt` — only when `--use-fixed-literals` is set
  and Exact emitted any ` c fixed <signed-int>` lines; one
  `xN 0|1` per fixed variable, in PRINTEMPS' `-f` format.

The PRINTEMPS phase writes `printemps_log.txt` and `printemps_log.stderr.log`.

## Signal handling

`SIGINT`, `SIGTERM`, and `SIGXCPU` are caught by the driver and forwarded to
the active child via `kill(pid, sig)`.  Children are placed in their own
process group so terminal Ctrl-C does not double-deliver.  Both Exact and
PRINTEMPS gracefully emit their best-known solution on these signals; the
driver waits for them to flush before exiting.
