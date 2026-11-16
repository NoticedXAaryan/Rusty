# Neural optimization protocol

Two untrusted paths sit beside the reference backend.

`rusty build --backend search` (and `rusty bench --suite FILE --compare`) searches RIR with equality saturation (`egg`) plus bounded partial evaluation. The e-graph covers algebraic cleanup, guarded-division elimination for a safe constant divisor, and closed forms for range-loop accumulators. A cost function extracts the term. It is not a trained model. The compare command reports against a 10% lower bound; the external generator's gate stays at 2%. The interpreter checks candidates that finish inside the step budget, and a native `-O0` differential check (stdout, stderr, exit status) runs before a rewrite may replace the reference binary. LLVM for this path is emitted only by the trusted backend, so the external sandbox is not required. A mismatch restores the reference. The research rationale is in [the RIR optimization note](research/rir-optimization.md). Public development measurements are in [measurements](research/measurements.md).

`rusty optimize <project>` and `rusty build <project> -Oai` always build the trusted `-O3` reference executable first. When a generator is configured, ordinary `build` and `run` also use the same safe neural-first path; without local configuration they remain reference-only. Neural optimization is attempted only when both local integration points are configured:

```text
RUSTY_NEURAL_GENERATOR=/path/to/generator
RUSTY_SANDBOX=/path/to/sandbox-wrapper
```

No project source or RIR is sent to a remote service by Rusty. The generator is an external local process invoked once per candidate:

```text
generator --input program.rir --output candidate-N.ll --target-cpu CPU --seed N
```

A successful generator must write one complete textual LLVM module. Rusty limits candidate size, requires a `main` definition, rejects inline assembly, raw integer/pointer casts, control-flow escape constructs, and external declarations other than the narrow runtime policy. Passing this policy is not formal verification.

The sandbox wrapper is invoked as:

```text
sandbox-wrapper executable [-- program arguments...]
```

It must isolate filesystem, network, process, memory, and CPU access; forward the program's stdout/stderr; and mirror its exit status. Rusty refuses to execute generated code without this explicitly configured wrapper.

For each candidate Rusty performs:

1. local generation with a bounded timeout;
2. static policy inspection;
3. Clang acceptance and native linking;
4. sandboxed differential output/exit-status comparison with the reference;
5. repeated sandboxed whole-program timing;
6. bootstrap speedup comparison and minimum-threshold gating.

Candidates are classified independently as neural win, reference win, statistical tie, neural incorrect, neural failed, or verifier rejected. The fastest statistically accepted candidate replaces the final executable only after the trusted executable is preserved. Installation failures restore the trusted executable. If configuration, generation, verification, correctness, or performance fails, Rusty keeps the reference binary and reports `REFERENCE_FALLBACK`.

This is the initial progressive verifier, not the final verification target. Generated tests, fuzzing, symbolic execution, translation validation, and independently reviewed platform sandboxes remain required before stable neural-first release claims.
