# Benchmark command

`rusty bench` measures the complete reference executable as a child process. It is intended for local regression checks and for validating measurement plumbing; private benchmark workloads, inputs, and results are not part of the public repository.

The runner uses a fixed protocol:

- build once with the trusted reference backend at `-O3`;
- execute three warm-up runs by default and discard them;
- collect thirty timed runs by default;
- retain every sample rather than deleting inconvenient measurements;
- report minimum, maximum, median, arithmetic mean, sample variance, sample standard deviation, a normal-approximation 95% confidence interval for the mean, and the number of Tukey-fence outliers;
- fail immediately if any warm-up or measured execution returns a nonzero status.

Override counts with `--warmup N` and `--repeat N`:

```text
cargo run --release -- bench examples/control --warmup 5 --repeat 50
```

Process startup and operating-system scheduling are included in every sample. Results therefore describe the tested whole executable on the current host; they do not establish universal performance and should not be presented as a comparison with another compiler without a frozen workload, equivalent inputs, machine controls, and independently recorded provenance.