# Rusty v0 ABI and reference backend

Rusty v0's public semantic ABI is target-independent: `int` is signed 64-bit, `bool` is a logical one-bit value, `string` is an immutable UTF-8 byte sequence terminated internally for C interoperation, and no-value functions return `unit`.

The trusted backend emits deterministic textual LLVM IR. Each source function receives a stable source-order numeric ID and the private symbol `rusty.fn.<id>`. Local IDs are assigned in deterministic declaration order. The generated host `main` calls the Rusty `main`; a unit result maps to process status 0 and an integer result is truncated to the platform's 32-bit C status value.

Native lowering uses the LLVM target selected by the installed `clang`. On the PRD's release target this means Linux x86-64 System V; this development build also works with Windows x86-64's host ABI. Rusty function signatures use:

| Rusty type | LLVM type |
|---|---|
| `int` | `i64` |
| `bool` | `i1` |
| `string` | opaque `ptr` |
| `unit` | `void` |

Locals use entry-block stack slots in v0. Calls and returns follow LLVM's target ABI lowering. Output uses the host C `printf`; generated format strings are fixed by the backend, never supplied by a Rusty program. Strings are passed read-only and are valid for the entire process.

Wrapping operations omit LLVM `nsw`/`nuw` flags. Division and remainder call guarded helpers that execute `llvm.trap` for zero divisors and `INT64_MIN / -1`, preventing LLVM undefined behavior. The v0 compiler does not expose raw pointers, allocation, references, arrays, foreign calls, threads, or shared mutable memory.

`clang` is the only external build requirement. Set `CLANG` to select an executable. Build failures are fatal and never silently reported as successful. Neural output has no ABI status in v0 because no neural backend is present; `-Oai` and `optimize` use an explicitly reported reference fallback.