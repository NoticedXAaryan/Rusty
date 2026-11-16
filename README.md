<p align="center">
  <img src="assets/logo-main.png" alt="Rusty logo" width="180">
</p>

# Rusty

Rusty is a small native language and a research compiler. The deterministic pipeline defines what a program means. Optional backends may propose a faster implementation, and nothing they produce is kept unless it matches the reference on the outputs that were actually run.

The research target is a measured 10% or better speedup over the reference Clang `-O2`/`-O3` pipeline. The direction is a JEPA-style world model that ranks semantics-preserving rewrites of canonical RIR, not a generator of LLVM text. The note is [docs/research/rir-optimization.md](docs/research/rir-optimization.md). The first slice in this tree is a verified rewrite search with a fixed linear cost model. No neural weights are shipped.

The language favors familiar, low-friction syntax—immutable `let` bindings, mutable `var` bindings, inferred local types, functions, conditions, and range loops—without requiring semicolons.

```rust
fn add(a: int, b: int) -> int {
    return a + b
}

fn main() {
    var score = 0
    for i in 1..5 {
        score += add(i, 1)
    }
    print("score", score)
}
```

## Compiler pipeline

```text
source → lexer → parser → name resolution → type checker
       → typed HIR → canonical RIR → LLVM IR → native executable
                         │
                         └─ optional search: fold, inline, affine summaries,
                            bounded partial evaluation
                            accept only if the interpreter and a native diff agree
```

The reference compiler has source-located diagnostics, deterministic name and type resolution, byte-stable RIR, guarded integer division, short-circuit logic, project discovery, and native linking through Clang. Private datasets, model weights, held-out suites, and raw result files stay out of git (`research/`, `benchmarks/`, `models/`, `results/`, and the usual weight and notebook globs).

## What the search measured

On the six public development kernels in `examples/kernels/` (not the private held-out suite), one release run with 5 warm-ups and 30 retained samples produced a geometric-mean speedup of **3.81×** versus reference `-O3` on this host (Intel Xeon, family 6 model 207, Clang 18.1.3). Four kernels clear a 10% lower confidence bound. Two do not: Clang already deletes the plain induction sum, and `hello` has no loop to remove. `-O2` is not faster than `-O3` here (geometric mean 0.99×).

| Kernel | Pipeline | -O3 median | Search median | Speedup | 95% CI | Clears 10% |
|---|---|---:|---:|---:|---|---|
| series | affine | 281570 ns | 258780 ns | 1.0881× | [1.0190, 1.2054] | no |
| modsum | affine | 2020153 ns | 274290 ns | 7.3650× | [7.1045, 7.6258] | yes |
| invdiv | affine | 4619414 ns | 282158 ns | 16.3718× | [15.1100, 17.1613] | yes |
| branch | eval | 1055590 ns | 271762 ns | 3.8842× | [3.4126, 4.2973] | yes |
| mix | eval | 1661012 ns | 277816 ns | 5.9788× | [4.9899, 6.2949] | yes |
| hello | identity | 265826 ns | 263019 ns | 1.0107× | [0.9423, 1.0826] | no |

Full host notes, the `-O2` column, and the reading of the table are in [docs/research/measurements.md](docs/research/measurements.md). Reproduce with:

```text
cargo run --release -- bench --suite examples/kernels/suite.tsv --compare --warmup 5 --repeat 30
```

## Build and run

Rusty requires Rust 1.93.1 and Clang 18 or newer. Set `CLANG` if Clang is not available on `PATH`.

```text
cargo build --release
cargo run -- check examples/hello
cargo run -- run examples/hello -O2
cargo run -- run examples/control -O3
cargo run -- test examples/control
cargo run --release -- bench examples/control --warmup 5 --repeat 50
cargo run --release -- bench --suite examples/kernels/suite.tsv --compare --warmup 5 --repeat 30
cargo run --release -- build examples/kernels/modsum --backend search -O3
cargo run -- research list
```

Create a project with `cargo run -- new hello`. A project contains `Rusty.toml`, `src/main.ry`, and optional standalone `tests/*.ry` programs. Builds place canonical `.rir`, generated `.ll`, and the native executable in the project's ignored `build/` directory.

## Language contracts

- [Language semantics](docs/language-v0.md)
- [Grammar](docs/grammar-v0.md)
- [Canonical RIR](docs/rir-v0.md)
- [ABI and reference backend](docs/abi-v0.md)
- [Conformance policy](docs/conformance-v0.md)
- [Benchmarking and RustyBench](docs/benchmarking.md)
- [Neural optimization protocol](docs/optimization.md)
- [Research direction: learned RIR optimization](docs/research/rir-optimization.md)
- [Public development measurements](docs/research/measurements.md)

## Development

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo build --workspace --all-targets --release
```

No trained model is claimed. The table above is the verified rewrite search, ranked by a reviewed linear prior, on public development kernels. The single-project `bench` command times the reference backend and honors `-O0`, `-O2`, and `-O3` (default `-O3`). Suite mode without `--compare` stays the reference `-O3` protocol: frozen checksums, leakage-resistant splits, retained samples. `rusty optimize` and `-Oai` still require a local external generator and `RUSTY_SANDBOX`; a failed or unconfigured generator keeps the reference binary.