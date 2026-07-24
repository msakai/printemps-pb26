# PRINTEMPS for Pseudo Boolean Competition 2026

[PRINTEMPS](https://snowberryfield.github.io/printemps/) solver for [Pseudo-Boolean Competition 2026](https://www.cril.univ-artois.fr/PB26/) submission.

It contains three versions of solvers:

- PRINTEMPS itself
- Hybrid solver that combines [Exact](https://gitlab.com/nonfiction-software/exact) and PRINTEMPS (see [README_exact_printemps.md](README_exact_printemps.md) for details)
- Hybrid solver that combines [SCIP](https://www.scipopt.org/) and PRINTEMPS (see [README_scip_printemps.md](README_scip_printemps.md) for details)

## Solver information

[Solver description](description/description.pdf)

### Solver suggested command line

PRINTEMPS itself:
```
DIR/bin/pb_competition_2025_solver -k -1 -t TIMEOUT -j NBCORE -r RANDOMSEED BENCHNAME
```

Exact + PRINTEMPS:
```
DIR/bin/exact-printemps --exact-path DIR/bin/Exact --printemps-path DIR/bin/pb_competition_2025_solver --save-dir TMPDIR -t TIMEOUT --seed RANDOMSEED -j NBCORE --use-fixed-literals BENCHNAME
```

SCIP + PRINTEMPS:
```
DIR/bin/scip-printemps --printemps-path DIR/bin/pb_competition_2025_solver --save-dir TMPDIR -t TIMEOUT --seed RANDOMSEED -j NBCORE --use-fixed-literals BENCHNAME
```

### Complete or not?
* ☐ Complete (your solver can answer UNSATISFIABLE)
* ☑ Incomplete (your solver can find solutions but cannot prove that there is no solution)

### VeriPB unchecked deletion
* ☐ use unchecked deletion mode for VeriPB (only relevant in the *-CERT tracks, for solvers generating UNSAT/OPT proofs)

### Categories of benchmarks

* ☑ DEC-LIN (decision problem, linear constraints, no UNSAT certificate)
* ☐ DEC-LIN-CERT (decision problem, linear constraints, UNSAT certificate required)
* ☑ DEC-NLC (decision problem, non-linear constraints, no UNSAT certificate)
* ☑ OPT-LIN (optimization problem, linear constraints, no OPT/UNSAT certificate)
* ☐ OPT-LIN-CERT (optimization problem, linear constraints, OPT/UNSAT certificate required)
* ☑ OPT-NLC (optimization problem, non-linear constraints, no OPT/UNSAT certificate)
* ☑ PARTIAL-LIN (WBO, both soft and hard constraints, linear constraints)
* ☑ SOFT-LIN (WBO, only soft constraints, linear constraints)

## Layout

| Path | Description |
|------|-------------|
| `src/`             | Rust driver source (`exact-printemps` and `scip-printemps`). |
| `Exact/`           | Submodule: Exact (AGPL-3.0). |
| `printemps/`       | Submodule: PRINTEMPS (MIT). |
| `build.sh`         | Builds the solvers and copies the resulting binaries to `bin/`. |
| `Cargo.toml`       | Cargo manifest for the driver. |
| `LICENSE`          | MIT license for the driver itself; bundled components keep their own licenses. |

## Build

```sh
git submodule update --init
BUILD_SCIP_PRINTEMPS=ON ./build.sh
```

After a successful build:

```
bin/
├── Exact                          # AGPLv3, sources under Exact/
├── pb_competition_2025_solver     # MIT, sources under printemps/
├── exact-printemps                # MIT, sources under src/
└── scip-printemps                 # MIT (links SCIP & SoPlex, both Apache-2.0)
```

The `scip-printemps` binary is opt-in via `BUILD_SCIP_PRINTEMPS=ON`; without
it, `build.sh` produces only the other three binaries.

By default (`STATIC=ON`), `build.sh` tries to link the binaries statically,
so the artifacts in `bin/` can be deployed to other Linux hosts regardless of
their installed shared libraries:

- `Exact` and `pb_competition_2025_solver` are linked with `-static`.
- `exact-printemps` is fully statically linked via Rust's `+crt-static`.
- `scip-printemps` links SCIP/SoPlex statically (via the `scip-from-source`
  Cargo feature, which builds SCIP from source with `-DSHARED=OFF`) but keeps
  glibc and libstdc++ (along with libgcc_s/libm) dynamic.

Run `STATIC=OFF ./build.sh` to use dynamic linking instead.
The script runs `file(1)` on each artifact at the end so you can confirm the
linkage mode.

### Build requirements

Common to all binaries (package names are Debian/Ubuntu examples; other
distributions ship equivalent packages):

- C and C++ compilers with their standard library headers (e.g. `gcc`, `g++`,
  `libc6-dev`, `libstdc++-*-dev`).
- CMake (>= 3.15) and Make.
- A Rust toolchain (`cargo`).

Per binary:

- `pb_competition_2025_solver` — OpenMP runtime (e.g. `libgomp1`); PRINTEMPS
  parallelizes with OpenMP.
- `Exact` — Boost headers >= 1.71 (e.g. `libboost-dev`).
- `scip-printemps` — see [README_scip_printemps.md](README_scip_printemps.md)
  for the supported SCIP build modes (`scip`, `scip-bundled`,
  `scip-from-source`) and their build-time prerequisites.

On Debian/Ubuntu, `build-essential cmake libboost-dev` covers all of the
above except the SCIP-specific prerequisites.

### Runtime requirements for `scip-printemps` (`scip-bundled` only)

When `scip-printemps` is built with `--features scip-bundled` (instead of
the default `scip-from-source` used by `build.sh`), the bundled `libscip.so`
needs the following at runtime:

- `libgfortran.so.5` (e.g. `libgfortran5`).
- `libquadmath.so.0` (e.g. `libquadmath0`). On Ubuntu 24.04 the `libgfortran5`
  package does not pull `libquadmath0` automatically.
- `LD_LIBRARY_PATH` must point at the directory containing the bundled
  `libscip.so` (it is linked without an rpath).

The `scip-from-source` build avoids both: SCIP is statically linked, and the
binary only retains the standard system libraries listed in the bullet above.

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
