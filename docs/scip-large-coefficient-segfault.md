# Investigation: `scip-printemps` segfault on large-coefficient PB instances

Status: **open / leading hypothesis identified, not yet confirmed on hardware.**
Last updated: 2026-06-15.

## TL;DR

- The submitted `scip-printemps` binary **segfaults inside SCIP's solve** on some
  PB instances on the competition machine, but the crash **cannot be reproduced**
  locally (Apple-Silicon Docker) or on GitHub Actions, and **valgrind is clean**.
- This is **not** the [`read_prob`/drop segfault](russcip-read_prob-drop-segfault.md):
  here `read_prob` succeeds and SCIP runs for ~2.5 s before crashing; coefficients
  are far below SCIP's `1e20` infinity.
- The submitted binary contains **zero AVX/AVX2/AVX512 instructions** (SSE2
  baseline only), so SCIP/SoPlex's own floating-point arithmetic — and therefore
  the branch-and-bound search path — is **identical** on every x86-64 CPU. That
  rules out an "FP-path differs by CPU" explanation.
- **Leading hypothesis:** the binary calls glibc string/memory functions
  (`memcpy`, `memmove`, `memset`, `memcmp`, `strlen`, …) that glibc **dispatches
  at run time via IFUNC to CPU-specific implementations**. The competition CPU
  (AMD EPYC 9355, Zen 5) has **AVX512**, so glibc selects the 64-byte AVX512
  variants; the GitHub Actions CPU (EPYC 7763, Zen 3) and the local emulator do
  not, so they select AVX2/SSE variants. A latent memory issue in SCIP can fault
  with the wider AVX512 access while the narrower AVX2 access survives.
- This also explains why **valgrind never reproduces it**: memcheck replaces the
  mem/str functions with its own implementations, so glibc's AVX512 variant is
  never executed.
- The competition's `run-glibc-2.39.sh` wrapper is a contributing factor (it
  supplies the glibc whose AVX512 routines run on Zen 5, and also swaps
  `libstdc++`/`libm`/`libc` to the competition's own copies), but the primary
  trigger appears to be **"AVX512 CPU × glibc IFUNC"**, not the wrapper alone.

## Affected artifact

| | |
|---|---|
| Binary | `scip-printemps` (submission `printemps_pb26/bin/scip-printemps`) |
| Driver commit | `94df829620dcc59d061675a7927d11cca65f2f3c`, built by `.github/workflows/ci.yml` |
| Rust deps | `russcip` 0.5.1, `scip-sys` 0.1.26 |
| SCIP | **9.2.4** (`scip-sys` `from-source` downloads `scipoptsuite-9.2.4.zip`), statically linked |
| Linkage | x86-64 PIE, dynamic; `NEEDED` = `libstdc++.so.6`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`, `ld-linux-x86-64.so.2`. **No `libgomp`** (no OpenMP). |
| Symbol requirements | up to `GLIBC_2.39` and `GLIBCXX_3.4.29` |

## The failure

Competition trace **4763789** (PB'26 PBS/PBO warmup), solver `scip-printemps 2026-05-18`:

- Instance: `PB24/SampleByIntSize/OPT-LIN-intsize=46.opb`
  (MD5 `84ad9497f18dc6395b3eff91b9f4d1cb`).
- Machine: **AMD EPYC 9355 (Zen 5)**; CPU flags include
  `avx512f avx512bw avx512vl avx512dq avx512ifma avx512vbmi …`.
- Invocation (via wrapper, see below):
  `./scip-printemps --printemps-path … --save-dir … -t 1800 --seed 1331088797 -j 1 --use-fixed-literals …/instance.opb`
- Solver output before the crash was a **single line**:
  `c scip-printemps: phase 1 (SCIP, budget=300.000s)`.
- Then at **2.57 s CPU / 3.04 s wall**:
  `./run-glibc-2.39.sh: line 2: … Segmentation fault (core dumped) …`,
  runsolver reports **`Child status: 139`** (= 128 + 11 = SIGSEGV).

Because only the phase-1 banner was printed (no `o`/`s`/`v`), the crash is inside
phase-1 SCIP — i.e. within `problem.solve()` in [`src/scip.rs`](../src/scip.rs)
(`russcip`'s `solve()` consumes the model, so the ~2.5 s of CPU is the SCIP solve
itself).

### Instance characteristics

`OPT-LIN-intsize=46.opb` header: `#variable= 14265 #constraint= 290 #equal= 78 intsize= 46`.

- Biggest objective coefficient: `714038312960` (40 bits).
- Sum of objective coefficients: `68224730472397` (≈ 6.8e13, 46 bits) → `intsize = 46`.
- The instance **is satisfiable** (competition metadata: best result SAT, best
  objective `1931531393`).
- Note `6.8e13 < 2^53 ≈ 9.0e15`, so the coefficients are **exactly representable
  in `f64`**. The problem is therefore *not* coefficient rounding.

## What it is **not**

The separate [`read_prob`/drop segfault](russcip-read_prob-drop-segfault.md)
requires a coefficient `>= 1e20` (SCIP's `numerics/infinity`), which makes SCIP's
OPB reader fail and the model's `drop` crash *before* solving. Here the largest
coefficient is `7.1e11`, `read_prob` succeeds, and SCIP solves for ~2.5 s before
faulting. **Different bug.**

## Reproduction attempts (all negative so far)

| Environment | CPU | glibc | AVX512? | Result |
|---|---|---|---|---|
| Competition | EPYC 9355 (Zen 5) | custom glibc-2.39 (wrapper) | **yes** | **SIGSEGV @ ~2.5 s** |
| Local Docker (OrbStack, Apple Silicon, `linux/amd64`) | emulated x86-64 (advertises only `sse4_2 fma`) | ubuntu:24.04 (2.39) | no | no crash; SCIP finds no incumbent, primal stuck at `1e20` |
| GitHub Actions `ubuntu-latest` (commit `32924f48…`) | EPYC 7763 (Zen 3) | `2.39-0ubuntu8.7` | no (AVX2 only) | no crash; valgrind clean |

All reproductions used the **exact submitted binary** and the **exact seed
`1331088797`** (the driver forwards it to SCIP `randomization/randomseedshift`,
so the search path is pinned).

### Behaviour sweep over `intsize` (local, exact seed)

Even where it does not crash, SCIP misbehaves on the larger members of this
family (driver-level observation):

| `intsize` | SCIP outcome |
|---|---|
| 41 | `o 394243446`, OPTIMUM (correct) |
| 43 | `o 1`, OPTIMUM |
| 45 | `o 1e20`, `s UNSATISFIABLE` (UNSAT is **correct** for this instance; the `o 1e20` line is spurious — see "secondary findings") |
| 46 | no incumbent; primal bound stuck at `1e20` (the crashing instance) |
| 47 | no incumbent; primal bound stuck at `1e20` |

## Binary analysis

Two facts (from `objdump`/`readelf` on the submitted binary) drive the current
hypothesis:

1. **No vectorized code of its own.** The binary contains **0** `ymm` (AVX/AVX2)
   and **0** `zmm` (AVX512) register references — SCIP/SoPlex was compiled at the
   SSE2 x86-64 baseline. Consequence: SoPlex's LP arithmetic produces **identical
   results on every x86-64 CPU**, so the branch-and-bound tree (which nodes are
   explored, in which order) is the **same** on Zen 3 and Zen 5. A crash that
   appears on one but not the other therefore cannot be explained by the solver's
   own floating point.

2. **It calls IFUNC-dispatched glibc functions.** Imported (undefined) symbols
   include `memcpy`, `memmove`, `memset`, `memcmp`, `bcmp`, `strlen`, `strcmp`,
   `strncmp`, `strchr`, `__memcpy_chk`, `__memset_chk`, `__strncpy_chk`. glibc
   resolves each of these **at load time** to a CPU-specific implementation.

## The competition wrapper

The binary is launched indirectly:

```
LD_LIBRARY_PATH=/home/evaluation/evaluation/priv/lib:/home/evaluation/evaluation/tools/lib64:\
/home/evaluation/evaluation/tools/lib:/usr/local/lib:/home/evaluation/evaluation/tools/glibc-2.39/lib/:/lib64 \
  /home/evaluation/evaluation/tools/glibc-2.39/lib/ld-linux-x86-64.so.2 $*
```

The binary requires `GLIBC_2.39`, so the competition ships a glibc 2.39 and runs
the binary through **its** dynamic loader, with `LD_LIBRARY_PATH` pointing at the
competition's library tree. This swaps not only `libc`/`ld-linux` but, via the
`tools/*` directories, potentially `libstdc++`, `libgcc_s`, and `libm` to the
competition's own copies — which are *different builds* from the GitHub Actions
stock libraries the binary was tested against.

## Leading hypothesis

Since SoPlex computes identically across x86-64 (SSE2 only), the difference that
makes Zen 5 crash must be in the **environment**, not the solver's math. The
strongest, CPU-tied candidate is the **glibc IFUNC mem/str routines**:

| | Competition EPYC 9355 (Zen 5) | GHA EPYC 7763 (Zen 3) / emulator |
|---|---|---|
| AVX512 | present | absent |
| glibc picks for `memmove`/`memset`/… | `__memmove_avx512_*`, `__memset_avx512_*`, `__*_evex*` (64-byte) | `__*_avx2_*` (32-byte) / SSE |

glibc 2.39 does ship these AVX512/EVEX implementations (verified present in
`2.39-0ubuntu8.7`'s `libc.so.6`). If SCIP has a latent memory issue — e.g. an
over-read of a correctly-sized buffer near a page boundary, or a slightly
out-of-bounds/incorrectly-sized access — the **64-byte AVX512 access can step
into an unmapped page (SIGSEGV) while the 32-byte AVX2 access stays in-bounds**.
This is a classic "crashes only on the newer CPU" signature and is fully
consistent with the data: identical search path everywhere, fault only where
glibc runs AVX512.

### Why valgrind shows nothing

valgrind/memcheck **replaces** `memcpy`/`memmove`/`memset`/`strlen`/… with its
own implementations, so the **glibc AVX512 variant is never executed under
valgrind**. A fault that lives in (or is triggered by) glibc's AVX512 routine is
therefore invisible to memcheck — exactly what was observed on GitHub Actions.

### Role of the wrapper (secondary, but real)

- The wrapper supplies the glibc-2.39 whose AVX512 routines run on Zen 5. Strictly
  this is a property of "AVX512 CPU × glibc", not the wrapper per se (GHA's stock
  glibc 2.39 would also use AVX512 on an AVX512 CPU). But if the competition's
  glibc-2.39 is a *different build* (e.g. vanilla upstream vs Ubuntu-patched
  `2.39-0ubuntu8.7`) with different `malloc`/tunable defaults and heap layout, the
  condition under which a latent bug faults can shift.
- The swapped **`libstdc++`** could change `std::sort` tie-breaking and
  `unordered_map` iteration order; SCIP uses STL containers in its heuristics, so
  in principle a different libstdc++ could change the search path and reach a
  crashing state only there. (Considered less likely than the glibc-IFUNC route,
  but not excluded.)
- A *gross* ABI mismatch from "swapped libc but not a dependency" is **unlikely**:
  the process ran ~2.5 s before crashing, whereas an ABI mismatch typically fails
  at load or crashes at t≈0. Worth a quick check that `libm` is not resolved from
  a non-2.39 directory (libc and libm must come from the same glibc).

## Secondary findings

### 1. Driver emitted SCIP's infinity sentinel as an objective (fixed)

`Model::obj_val()`/`best_bound()` return SCIP's `numerics/infinity` (default
`1e20`, a **finite** `f64`) when a bound is absent. The old guard in
[`src/scip.rs`](../src/scip.rs) used only `is_finite()`, so on instances where
SCIP found no incumbent (e.g. `intsize=46/47`) the driver printed a bogus
`o 100000000000000000000`. Fixed in **PR #27** (`finite_bound(value, infinity)`
rejects values at/beyond SCIP's infinity). This is an output-correctness fix and
does **not** address the crash.

### 2. The `--scip-max-intsize` mitigation does not cover the crash

`DEFAULT_SCIP_MAX_INTSIZE = 53` ([`src/bin/scip_printemps.rs`](../src/bin/scip_printemps.rs))
is justified by the `2^53` f64 exact-integer limit, but the crashing instance is
`intsize=46` and the "no incumbent / spurious infinity" cases are `intsize=45–47`
— **all ≤ 53, so none are skipped**. The empirical failure boundary is lower than
53 and is also instance-structure dependent. Because the crash happens *inside*
`solve()` (before any result is returned), **only pre-skipping SCIP can prevent
it** — after-the-fact verification cannot.

## Proposed experiments (cheapest / most decisive first)

1. **Force glibc off AVX512 on the competition machine** (1 line, very decisive).
   Prepend to the wrapper:
   ```
   GLIBC_TUNABLES=glibc.cpu.hwcaps=-AVX512F,-AVX512VL,-AVX512BW
   ```
   This makes glibc select AVX2/SSE mem/str implementations. (Tunable confirmed
   present and accepted by glibc 2.39.) **If the crash disappears, the glibc
   AVX512 routines are confirmed as the trigger.**

2. **Reproduce on any AVX512 box** (AMD Zen 4/Zen 5 or Intel Ice Lake+ cloud VM)
   with the same wrapper and `--seed 1331088797`. GHA `ubuntu-latest` is Zen 3 and
   cannot trigger it.

3. **Inspect the crash with gdb** — no rebuild needed. Use the competition core
   dump (the trace says "core dumped") or a fresh repro on an AVX512 box with
   `ulimit -c unlimited`:
   ```
   gdb scip-printemps core
   (gdb) bt
   (gdb) x/i $pc
   ```
   If `$pc` is inside `__memmove_avx512_*` / `__memset_avx512_*` / `__*_evex*` and
   the fault address is ~1 page past a heap buffer, the AVX512 over-read theory is
   directly confirmed.

4. **AddressSanitizer build of standalone SCIP 9.2.4** (catches a genuine
   OOB/UAF). The bug is in C/C++ SCIP, so ASan must instrument the SCIP build, not
   the Rust crate; `scip-sys` hardcodes `SANITIZE_ADDRESS=OFF`, so bypass Rust and
   build SCIP directly:
   ```bash
   cmake -S scipoptsuite-9.2.4 -B build-asan \
     -DCMAKE_BUILD_TYPE=RelWithDebInfo -DSANITIZE_ADDRESS=on \
     -DSHARED=off -DSYM=snauty \
     -DGMP=off -DZLIB=off -DIPOPT=off -DZIMPL=off -DREADLINE=off -DPAPILO=off -DBOOST=off
   cmake --build build-asan -j --target scip
   ASAN_OPTIONS=abort_on_error=1:detect_leaks=0 \
     build-asan/bin/scip -c "set randomization randomseedshift 1331088797
                             set limits time 300
                             read OPT-LIN-intsize=46.opb
                             optimize
                             quit"
   ```
   This `scip` invocation mirrors what the driver does (`limits/time` +
   `randomseedshift` + solve). **Caveats:** still needs an AVX512 box; and ASan
   changes heap layout (redzones), so a purely *layout-sensitive* page-boundary
   over-read may not reproduce under ASan (a different blind spot from valgrind) —
   but a genuine heap-buffer-overflow / use-after-free will be caught precisely.
   If ASan is clean where the un-instrumented binary crashes, that itself points
   back to experiment (1).

## Robustness fixes (independent of root-cause confirmation)

- **A. Skip SCIP on these instances.** Lower/replace the `--scip-max-intsize`
  threshold so `intsize=46` (and the rest of the failing `SampleByIntSize` family)
  is pre-skipped; SCIP adds little on these near-pure-encoding instances anyway,
  and pre-skip is the only thing that can prevent the in-`solve()` crash.
- **B. Eliminate the wrapper dependency.** Build against an older glibc (e.g.
  ubuntu 20.04 / manylinux) so the binary does not require `GLIBC_2.39`; the
  competition then need not swap glibc/`libstdc++`/`libm`, removing that whole
  class of variables.
- **C. Insurance: disable glibc AVX512 mem/str routines from inside the driver.**
  Have the driver set `GLIBC_TUNABLES=glibc.cpu.hwcaps=-AVX512F` and re-exec
  itself at startup. If experiment (1) confirms the cause, this stops the crash
  even while still running SCIP.

## Appendix: confirmed vs. hypothesized

- **Confirmed:** crash is SIGSEGV inside phase-1 `solve()`; not the read_prob/drop
  bug; binary is SSE2-only (no AVX/AVX2/AVX512); binary calls IFUNC mem/str
  functions; competition CPU has AVX512, GHA/emulator do not; SCIP misbehaves
  (no incumbent / spurious `1e20`) on `intsize≥45` even without crashing; the
  `--scip-max-intsize=53` default does not cover the failing instances; the
  `1e20`-as-objective driver bug (fixed in PR #27).
- **Hypothesized (not yet hardware-confirmed):** the segfault is triggered by
  glibc's AVX512 IFUNC mem/str implementation on Zen 5 exposing a latent SCIP
  memory issue; the wrapper's library swap is a secondary contributor. The
  experiments above are designed to confirm or refute this.
</content>
</invoke>
