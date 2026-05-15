# PRINTEMPS for Pseudo Boolean Competition 2026

[PRINTEMPS](https://snowberryfield.github.io/printemps/) solver for [Pseudo-Boolean Competition 2026](https://www.cril.univ-artois.fr/PB26/) submission.

It contains two versions of solvers:

- PRINTEMPS itself
- Hybrid solver that combines [Exact](https://gitlab.com/nonfiction-software/exact) and PRINTEMPS (see [README_hybrid.md](README_hybrid.md) for details)

## Solver information

### Solver suggested command line

PRINTEMPS itself:
```
DIR/bin/pb_competition_2025_solver -k -1 -t TIMEOUT -j NBCORE -r RANDOMSEED BENCHNAME
```

Hybrid solver (Exact + PRINTEMPS):
```
DIR/bin/exact-printemps --exact-path DIR/bin/Exact --printemps-path DIR/bin/pb_competition_2025_solver --save-dir TMPDIR -t TIMEOUT --seed RANDOMSEED -j NBCORE BENCHNAME
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
| `src/`             | Rust driver source (`exact-printemps`). |
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
└── exact-printemps                # MIT, sources under src/
```

By default `build.sh` links all three binaries statically (Exact and
`pb_competition_2025_solver` via `-static`, `exact-printemps` via Rust's
`+crt-static`) so the artifacts in `bin/` can be copied to other Linux
hosts without matching the build host's libc/libstdc++/libgomp versions.
The build needs the corresponding static archives (e.g. `libc6-dev`,
`libstdc++-*-dev`, `libgomp1` on Debian/Ubuntu, plus `libboost-dev` for
Exact). Pass `STATIC=OFF ./build.sh` to fall back to dynamic linking if
those archives are unavailable.

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
