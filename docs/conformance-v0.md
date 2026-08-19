# Rusty v0 conformance

The v0 conformance suite is the executable interpretation of the public language, RIR, and ABI contracts. It lives in `tests/conformance.rs` and must pass in full before a release can claim support for Rusty v0.

The suite checks four boundaries:

1. **Acceptance:** every documented v0 construct reaches typed HIR.
2. **Rejection:** unsupported or ill-typed programs fail with the required diagnostic class.
3. **Native behavior:** generated LLVM is compiled by Clang and its output is compared byte-for-byte after platform newline normalization.
4. **Runtime failure:** guarded arithmetic traps and nonzero `main` results remain observable.

Covered behavior includes scalar types, function calls and returns, lexical shadowing, mutation, conditionals, half-open and descending ranges, precedence, signed division and remainder, wrapping arithmetic, UTF-8 strings, short-circuit evaluation, argument evaluation order, process status, and arithmetic traps.

Run the complete suite with:

```text
cargo test --workspace --all-targets
```

Clang 18 or newer is required for native conformance cases. The suite fails rather than silently skipping native tests when Clang is unavailable. Set `CLANG` to test a particular installation.

A test count is not itself proof of language correctness. Adding a language feature requires updating its normative specification and adding acceptance, rejection, RIR, and native cases as applicable. The supported v0 subset passes only when every checked case passes through the trusted reference pipeline.