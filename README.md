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
                         └─ optional search: egg equality saturation
                            (algebra, unguarded div, loop closed forms)
                            plus bounded partial evaluation
                            accept only if the interpreter and a native diff agree
```

The reference compiler has source-located diagnostics, deterministic name and type resolution, byte-stable RIR, guarded integer division, short-circuit logic, project discovery, and native linking through Clang. Private datasets, model weights, held-out suites, and raw result files stay out of git (`research/`, `benchmarks/`, `models/`, `results/`, and the usual weight and notebook globs).

## What the search measured

On the six frozen public development kernels in `examples/kernels/` (not a private held-out suite), the **confirmation** run is what counts: 5 warm-ups, 30 retained samples, link order and environment size randomized, after a separate discovery timing that is not quoted. Host: Intel Xeon, family 6 model 207, Clang 18.1.3. Geometric mean versus reference `-O3` is **3.708×**. Versus `-O3 -march=native` it is **2.349×**. Four kernels clear a 10% lower bound against both baselines. `series` and `hello` do not. `-O2` versus `-O3` is noise here (geometric mean 1.004×). Alive2 was not on `PATH`, so these rewrites are not formally translation-validated.

| Kernel | Pipeline | -O3 median (ns) | Search (ns) | vs -O3 | 95% CI | Clears 10% | vs `-march=native` | Clears 10% |
|---|---|---:|---:|---:|---|---|---:|---|
| series | eqsat | 289542.5 | 292687.5 | 0.9893× | [0.9315, 1.0816] | no | 0.9663× | no |
| modsum | eqsat | 2099153.0 | 313608.5 | 6.6935× | [5.9583, 7.0177] | yes | 3.1426× | yes |
| invdiv | eqsat | 4698783.5 | 294608.0 | 15.9493× | [13.6855, 16.4068] | yes | 7.7675× | yes |
| branch | eval | 1166831.5 | 307025.0 | 3.8004× | [3.4367, 4.0300] | yes | 2.4075× | yes |
| mix | eval | 1642481.5 | 280373.5 | 5.8582× | [5.5243, 6.0140] | yes | 3.5494× | yes |
| hello | identity | 306362.5 | 277353.5 | 1.1046× | [0.9914, 1.4830] | no | 0.8337× | no |

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