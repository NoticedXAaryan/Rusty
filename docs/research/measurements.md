# Public development measurements

These numbers are the confirmation timing of `examples/kernels/suite.tsv`. The suite manifest is pinned by a checksum test. They are not a RustyBench held-out result, and no trained model was used. A discovery timing ran first, with a different layout seed, and was not used to change the compiler or to choose which kernels to keep. The discovery geometric mean versus `-O3` was 3.455×. It is not the result below.

## Protocol

- Rust 1.93.1 (`01f6ddf75`, 2026-02-11), Ubuntu Clang 18.1.3, Linux 6.12.113+ x86-64.
- Host `/proc/cpuinfo`: Intel(R) Xeon(R) Processor, family 6, model 207, stepping 2, 4 CPUs, `cpu MHz` 2400.000. The harness cpufreq fields were null.
- Command: `cargo run --release -- bench --suite examples/kernels/suite.tsv --compare --warmup 5 --repeat 30`
- Baselines: reference Clang `-O2`, `-O3`, and `-O3 -march=native`. The search binary is compiled at `-O3` and again at `-O3 -march=native`.
- Each sample links one of four padding objects (size and order vary) and sets `RUSTY_LAYOUT_PAD` to a different length. Whole-process time includes startup.
- The quoted rows are the second timing (confirmation). Speedup is baseline median / candidate median. "Clears 10%" means the bootstrap 95% lower bound is at least 1.10. The external generator's gate is still 2% and is not what this table uses.
- `alive-tv` was not on `PATH`. Equivalence for this run is the interpreter, a native stdout/stderr/status diff, and the differential fuzzer in `src/eqsat.rs` (80 random programs). That is not an Alive2 proof.

## Confirmation results

| Kernel | Pipeline | -O2 (ns) | -O3 (ns) | -O3 native (ns) | Search -O3 (ns) | Search native (ns) | vs -O3 | 95% CI | Clears 10% | vs native | 95% CI | Clears 10% |
|---|---|---:|---:|---:|---:|---:|---:|---|---|---:|---|---|
| series | eqsat | 304174.5 | 289542.5 | 296407.5 | 292687.5 | 306753.0 | 0.9893× | [0.9315, 1.0816] | no | 0.9663× | [0.8683, 1.0407] | no |
| modsum | eqsat | 2068875.5 | 2099153.0 | 1035959.0 | 313608.5 | 329649.0 | 6.6935× | [5.9583, 7.0177] | yes | 3.1426× | [2.6033, 3.3675] | yes |
| invdiv | eqsat | 4668122.0 | 4698783.5 | 2662557.5 | 294608.0 | 342780.0 | 15.9493× | [13.6855, 16.4068] | yes | 7.7675× | [6.5918, 8.6292] | yes |
| branch | eval | 1161644.0 | 1166831.5 | 725752.0 | 307025.0 | 301455.5 | 3.8004× | [3.4367, 4.0300] | yes | 2.4075× | [2.2000, 2.8806] | yes |
| mix | eval | 1629346.0 | 1642481.5 | 1031921.0 | 280373.5 | 290731.0 | 5.8582× | [5.5243, 6.0140] | yes | 3.5494× | [3.0919, 3.7917] | yes |
| hello | identity | 293784.5 | 306362.5 | 282563.0 | 277353.5 | 338908.5 | 1.1046× | [0.9914, 1.4830] | no | 0.8337× | [0.7580, 0.9092] | no |

Geometric means on the confirmation run:

- search `-O3` / reference `-O3`: **3.708×**
- search `-O3 -march=native` / reference `-O3 -march=native`: **2.349×**
- reference `-O2` / reference `-O3`: **1.004×**

## How to read this

`modsum` and `invdiv` are loops the e-graph replaced with a constant. `branch` and `mix` are closed programs that partial evaluation residualized; equality saturation did not find a closed form, and that residual will not survive once programs take input. `series` is an induction sum Clang already deletes, and the confirmation interval crosses 1. `hello` has nothing to rewrite; against `-march=native` the search binary was slower, and the `-O3` interval crosses 1.

`-O2` and `-O3` agree on this set, which is what Stabilizer predicts for a small scalar suite once layout moves. The geometric means are carried by four development kernels. They are evidence that the RIR rewrites beat this lowering. They are not a 10% claim on general code, and they are not a JEPA result.
