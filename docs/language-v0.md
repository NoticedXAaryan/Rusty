# Rusty language v0

Rusty v0 is a deterministic, UTF-8 source language. Newlines and optional semicolons separate statements. Comments begin with `//`. A program contains typed functions; `main` takes no parameters and returns either nothing or `int`.

Types are `int` (signed 64-bit), `bool`, `string`, and the implicit no-value type. Parameters and function results are annotated; local `let`/`var` declarations are inferred. `let` is immutable and `var` is mutable. Names have lexical block scope and may shadow an outer local.

Statements are declarations, `=`, `+=`, `-=`, calls, `return`, `if`/`else`, and `for name in start..end`. Ranges are ascending and half-open. Both bounds are evaluated once; an empty or descending range executes zero iterations. The loop variable is immutable.

Expressions include literals, calls, parentheses, unary `-`/`!`, arithmetic `+ - * / %`, comparison `== != < <= > >=`, and boolean `&& ||`. Precedence is conventional. Evaluation is left-to-right and `&&`/`||` short-circuit. User calls use exact argument types. `print` accepts any number of scalar values, separates them with one space, and writes a newline.

Integer `+`, `-`, `*`, unary negation, loop increments, `+=`, and `-=` wrap modulo 2^64. Comparisons are signed. Division truncates toward zero and remainder has the dividend's sign. Division or remainder by zero, and `INT64_MIN / -1` or `INT64_MIN % -1`, terminate through the backend trap instead of invoking undefined behavior.

A non-void function must return on every control-flow path. Assignment requires an existing mutable local. Conditions must be boolean. Strings are immutable UTF-8 values and v0 permits them as values, parameters, results, and print arguments; string comparison is not part of v0.
