# RIR v0

RIR v0 is Rusty's canonical, typed, structured reference IR. It is generated only after name resolution and type checking. Functions are ordered as in source; parameters and locals use deterministic numeric IDs. Every expression carries an explicit type.

The canonical serializer is an S-expression format headed by `(rir v0 ...)`. Strings use JSON-style escapes, integers are decimal, booleans are `true`/`false`, and indentation is two spaces. There are no addresses, timestamps, host paths, hash-map iteration results, or target-dependent names in serialized output.

Structured instructions are `let`, `set`, `return`, `expr`, `if`, and `for`. Expressions are `int`, `bool`, `string`, `local`, `unary`, `binary`, and `call`. `for` retains half-open range semantics. Arithmetic operations retain the wrapping and guarded division semantics defined by language-v0.

RIR is a semantic boundary, not LLVM IR. The trusted textual LLVM backend lowers RIR deterministically, using stack slots for resolved locals and explicit branches for structured control flow. Neural output is unavailable in this milestone; callers must use and report the reference fallback rather than imply neural compilation occurred.
