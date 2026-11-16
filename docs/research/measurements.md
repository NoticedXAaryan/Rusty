# Public development measurements

These numbers are from one run of the public kernels in `examples/kernels/suite.tsv`. They are not a RustyBench held-out result. The private suite is not in this checkout, and no trained model was used. The search ranked a fixed linear cost model and kept a rewrite only after the interpreter and a native differential check agreed with the reference.

## Protocol

- Rust 1.93.1 (`01f6ddf75`, 2026-02-11), Ubuntu Clang 18.1.3, Linux 6.12.113+ x86-64.
- Host `/proc/cpuinfo`: Intel(R) Xeon(R) Processor, family 6, model 207, stepping 2, 4 CPUs, `cpu MHz` 2400.000. The harness cpufreq fields were null on this host.
- Command: `cargo run --release -- bench --suite examples/kernels/suite.tsv --compare --warmup 5 --repeat 30`
- Each cell is a whole-process median over 30 retained samples after 5 discarded warm-ups. Speedup is `median(-O3) / median(candidate)`. The interval is the harness's deterministic bootstrap 95% interval on that ratio. "Clears 10%" means the lower bound is at least 1.10.
- `-O2` and `-O3` compile the reference RIR lowering. `search` compiles the selected RIR pipeline at `-O3`.

## Results

| Kernel | Pipeline | -O2 median (ns) | -O3 median (ns) | Search median (ns) | Speedup vs -O3 | 95% CI | Clears 10% |
|---|---|---:|---:|---:|---:|---|---|
| series | affine | 277528 | 281570 | 258780 | 1.0881 | [1.0190, 1.2054] | no |
| modsum | affine | 2090067 | 2020153 | 274290 | 7.3650 | [7.1045, 7.6258] | yes |
| invdiv | affine | 4599888 | 4619414 | 282158 | 16.3718 | [15.1100, 17.1613] | yes |
| branch | eval | 1141756 | 1055590 | 271762 | 3.8842 | [3.4126, 4.2973] | yes |
| mix | eval | 1610980 | 1661012 | 277816 | 5.9788 | [4.9899, 6.2949] | yes |
| hello | identity | 266190 | 265826 | 263019 | 1.0107 | [0.9423, 1.0826] | no |

Geometric mean of the six search/`-O3` speedups: **3.814**. Geometric mean of the six `-O2`/`-O3` speedups: **0.989** (`-O2` is not faster than `-O3` on this set).

## How to read this

`modsum`, `invdiv`, `branch`, and `mix` are loops Clang 18 still executes after `-O3`. The search replaces them with a residual print. Their lower bounds clear 10% on this host. `series` is an induction sum Clang already deletes; the residual is a few tens of microseconds faster and the lower bound does not clear 10%. `hello` has nothing to rewrite; the interval crosses 1.0.

The geometric mean is carried by those four loops. It is evidence that the rewrite interface can beat this reference lowering, not evidence that a JEPA has been trained, and not a result on `final-held-out`. Repeating the command on another machine will not reproduce these medians exactly. The pipelines and the pass/fail of the 10% lower bound are the claim to re-check.
