# Benchmarking and RustyBench

`rusty bench <project>` measures one complete trusted-reference executable as a child process. It is a local regression aid, not a comparative performance claim.

`rusty bench --suite <manifest>` runs a frozen private RustyBench manifest. Benchmark projects, inputs, model data, and results are deliberately excluded from the public repository. A manifest begins with `rustybench-v0`; each following tab-separated row contains:

```text
id  category  split  project  source_fnv1a64  expected_stdout_fnv1a64  [arguments]
```

Allowed splits are `train`, `validation`, `development`, `final-held-out`, and `case-study`. The loader rejects duplicate IDs and any project reused across splits. Categories cover microbenchmarks, algorithms, systems kernels, memory, branch-heavy, numerical, applications, generated programs, and DOOM. Frozen source checksums are checked before execution, and normalized-output checksums are checked on every oracle, warm-up, and measured run.

The fixed measurement behavior is:

- build once with the trusted reference backend at `-O3`;
- execute three discarded warm-ups and thirty measured runs by default;
- retain every raw sample and count Tukey-fence outliers without deleting them;
- report min/max, median, arithmetic mean, sample variance/deviation, a deterministic bootstrap 95% interval for the median, and a diagnostic normal-approximation interval for the mean;
- record CPU frequency before and after where the host exposes Linux cpufreq data;
- fail on source drift, output drift, launch failure, or nonzero status;
- aggregate per-workload medians geometrically only for a scale summary, never as a substitute for relative baseline speedups.

Example private run:

```text
rusty bench --suite benchmarks/suite.tsv --split final-held-out --warmup 5 --repeat 50 --output results/raw/run.bench.json
```

The JSON record retains all samples, checksums, split/category labels, host architecture, frequency observations, and protocol counts. Files ending in `.bench.json` and the standard private trees are ignored by Git.

Comparative candidate decisions use bootstrap confidence intervals over reference/candidate speedup distributions. Results are classified as neural win, reference win, statistical tie, neural incorrect, neural failed, verifier rejected, or fallback used. A candidate may pass a performance gate only when it is a statistically supported win and the lower confidence bound exceeds the configured minimum speedup.

Process startup and operating-system scheduling are included. Controlled final experiments must additionally freeze workloads, inputs, compiler versions/flags, fair GCC/Clang/native/LTO/PGO baselines where applicable, stopping criteria, environmental controls, and an independent reproduction. No benchmark result should be generalized beyond the recorded host and protocol.

`rusty bench --suite FILE --compare` is the research measurement, not the reference-only suite mode above. It builds Clang `-O2`, `-O3`, and `-O3 -march=native`, plus the in-tree equality-saturation pipeline at `-O3` and at `-O3 -march=native`. Each sample links a different padding object (size and link order) and sets `RUSTY_LAYOUT_PAD` to a different length, so a single layout cannot masquerade as a speedup. The command times the set twice. The second timing is the reported run; the first is discovery and is kept in the JSON only. The external generator's performance gate still defaults to 2%. The compare report's "clears 10%" column uses 1.10.
