# Bug report: segfault when `Model` is dropped after `read_prob` fails

This problem is reported as https://github.com/scipopt/russcip/issues/281 .

- **Crate:** `russcip` — found on 0.5.1 (with `scip-sys` 0.1.26); **also confirmed
  present in 0.9.1, the latest release** (see "Status in 0.9.1" below).
- **Symptom:** Segmentation fault (use-after-free / refcount underflow) when a
  `Model` is dropped after `read_prob` returns `Err`.
- **Trigger in the wild:** Reading an OPB instance whose objective/constraints
  contain a coefficient `>= SCIP's numerics/infinity (default 1e20)`. SCIP's OPB
  reader rejects it with `SCIP_INVALIDDATA (-9)`, `read_prob` returns `Err`, and the
  subsequent drop of the model crashes on some SCIP builds (e.g. a statically linked
  SCIP built from source).

## Summary

`Model::read_prob` (typestate API) consumes the model and, on the SCIP error path,
drops it. Dropping the model runs `ScipPtr::drop`, which **releases** every original
SCIP variable and constraint (`SCIPreleaseVar` / `SCIPreleaseCons`). But the matching
**captures** (`SCIPcaptureVar` / `SCIPcaptureCons`) are performed only *after*
`SCIPreadProb` succeeds — so when `SCIPreadProb` fails midway, the variables/constraints
SCIP created during the partial read are released without ever being captured. The
resulting refcount underflow frees those objects prematurely, and the final `SCIPfree`
(or the release loop itself) dereferences freed memory → **segfault**.

Whether it crashes is SCIP-version/build dependent (it depends on the stage SCIP is
left in after the reader error and on the original vars/conss still present), which is
why some builds report the error cleanly while others segfault.

## Root cause (source references, russcip 0.5.1)

`src/scip.rs` — `read_prob` captures vars/conss only on success:

```rust
pub(crate) fn read_prob(&self, filename: &str) -> Result<(), Retcode> {
    let filename = CString::new(filename).unwrap();
    scip_call!(ffi::SCIPreadProb(           // <-- returns Err early on SCIP_INVALIDDATA
        self.raw,
        filename.as_ptr(),
        std::ptr::null_mut()
    ));
    // capture vars and cons since they were not created by the user
    self.vars(true);    // <-- SKIPPED on the error path (never reached)
    self.conss(true);   // <-- SKIPPED on the error path (never reached)
    Ok(())
}
```

`src/model.rs` — on `Err`, the consumed model is dropped:

```rust
pub fn read_prob(mut self, filename: &str) -> Result<Model<ProblemCreated>, Retcode> {
    let scip = self.scip.clone();
    scip.read_prob(filename)?;   // <-- on Err, `self` is dropped here
    let new_model = Model { scip: self.scip, state: ProblemCreated {} };
    Ok(new_model)
}
```

`src/scip.rs` — `ScipPtr::drop` releases all original vars/conss whenever SCIP is in a
problem/solving stage, regardless of whether they were ever captured:

```rust
impl Drop for ScipPtr {
    fn drop(&mut self) {
        ...
        let scip_stage = unsafe { ffi::SCIPgetStage(self.raw) };
        if scip_stage == ffi::SCIP_Stage_SCIP_STAGE_PROBLEM
            || ... {
            // release original variables
            let n_vars = unsafe { ffi::SCIPgetNOrigVars(self.raw) };
            let vars = unsafe { ffi::SCIPgetOrigVars(self.raw) };
            for i in 0..n_vars {
                let mut var = unsafe { *vars.add(i as usize) };
                scip_call_panic!(ffi::SCIPreleaseVar(self.raw, &mut var));  // <-- unbalanced
            }
            ...
            // release constraints (same unbalanced release for SCIPgetOrigConss)
        }
        unsafe { ffi::SCIPfree(&mut self.raw) };
    }
}
```

`vars(true)` / `conss(true)` are the only place captures happen for reader-created
objects:

```rust
pub(crate) fn vars(&self, capture: bool) -> BTreeMap<usize, *mut SCIP_Var> {
    ...
    if capture { unsafe { ffi::SCIPcaptureVar(self.raw, scip_var); } }
    ...
}
```

### The imbalance

| Path                        | captures done? | drop releases? | result            |
|-----------------------------|----------------|----------------|-------------------|
| `read_prob` success         | yes (vars/conss)| yes            | balanced, OK      |
| `read_prob` failure (SCIP_INVALIDDATA) | **no** | **yes**        | over-release → UAF / segfault |

## Status in 0.9.1 (latest)

Verified against the russcip 0.9.1 source: **the bug is unchanged.** Both halves of
the imbalance are still present (line numbers are 0.9.1):

- `read_prob` still captures only on the success path — `src/model.rs:97` drops the
  consumed `Model` on `Err`, and `src/scip.rs:149` runs the captures *after* the
  fallible call:

  ```rust
  pub(crate) fn read_prob(&self, filename: &str) -> Result<(), Retcode> {
      let filename = CString::new(filename).unwrap();
      scip_call!(ffi::SCIPreadProb(self.raw, filename.as_ptr(), std::ptr::null_mut())); // early Err
      self.vars(false, true);  // SKIPPED on the error path
      self.conss(true);        // SKIPPED on the error path
      Ok(())
  }
  ```

- `ScipPtr::drop` (`src/scip.rs:1746`) still unconditionally releases all
  `SCIPgetOrigVars` / `SCIPgetOrigConss` when SCIP is in `PROBLEM` (or later) stage —
  identical to 0.5.1.

So upgrading to 0.9.1 alone does **not** fix the crash; the issue needs an upstream
fix (or the downstream workaround below).

## Reproduction

`bad.opb` (coefficient `2^67 = 147573952589676412928 >= 1e20`):

```
+147573952589676412928 x1 >= 1 ;
```

```rust
use russcip::prelude::*;

fn main() {
    let model = Model::new().hide_output().include_default_plugins();
    let res = model.read_prob("bad.opb"); // Model consumed; dropped on Err
    // SCIP prints:
    //   [cons_linear.c] ERROR: coefficient of variable <x1> is infinite.
    //   [reader_opb.c]  ERROR: Error <-9> ...
    assert!(res.is_err());
    // On some SCIP builds the process segfaults during the drop above,
    // before this point is reached.
}
```

## Suggested fixes (upstream)

Any of the following would remove the imbalance:

1. **In `read_prob`, on the `SCIPreadProb` error path, free/transform the model back to
   a clean stage** (e.g. call `SCIPfreeProb`) before returning `Err`, so `Drop` finds no
   original vars/conss to release.
2. **Track whether vars/conss were captured** and have `Drop` release only what was
   actually captured (don't release `SCIPgetOrigVars` if no capture happened).
3. **Capture before the fallible call / in a guard**, or capture incrementally so the
   capture/release sets always match even on partial reads.

## Workaround (downstream)

Avoid handing SCIP an instance whose reader will fail with `SCIP_INVALIDDATA` — e.g.
pre-scan the OPB and skip the SCIP phase when a coefficient `>= 1e20` is present (the
smallest 21-digit integer), so `read_prob` is never called on a rejectable instance.
Catching the returned `Err` is **not** sufficient, because the crash happens inside the
drop during `read_prob`, before the `Err` is observed.
