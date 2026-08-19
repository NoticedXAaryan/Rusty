use rusty_lang::{analyze, compile};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

fn temp_directory() -> PathBuf {
    let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("rusty-conformance-{}-{id}", std::process::id()))
}

fn executable_path(directory: &Path) -> PathBuf {
    directory.join(if cfg!(windows) { "case.exe" } else { "case" })
}

fn compile_and_run(name: &str, source: &str) -> Output {
    let (_, llvm) = compile(name, source)
        .unwrap_or_else(|errors| panic!("{name} failed frontend validation:\n{}", errors[0]));
    let directory = temp_directory();
    fs::create_dir_all(&directory).expect("create conformance directory");
    let input = directory.join("case.ll");
    let executable = executable_path(&directory);
    fs::write(&input, llvm).expect("write LLVM fixture");
    let clang = std::env::var_os("CLANG").unwrap_or_else(|| "clang".into());
    let compiler = Command::new(clang)
        .arg("-O0")
        .arg(&input)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("Clang is required for native conformance tests");
    assert!(
        compiler.status.success(),
        "Clang rejected {name}:\n{}",
        String::from_utf8_lossy(&compiler.stderr)
    );
    let output = Command::new(&executable)
        .output()
        .expect("run conformance binary");
    fs::remove_dir_all(directory).expect("remove conformance directory");
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .expect("Rusty output is UTF-8")
        .replace("\r\n", "\n")
}

#[test]
fn accepts_the_supported_language_surface() {
    let cases = [
        ("empty", "fn main() {}"),
        (
            "types",
            "fn main() { let i = 1\nlet b = true\nlet s = \"text\"\nprint(i, b, s) }",
        ),
        (
            "function",
            "fn add(a: int, b: int) -> int { return a + b }\nfn main() { print(add(1, 2)) }",
        ),
        (
            "shadow",
            "fn main() { let x = 1\nif true { let x = 2\nprint(x) }\nprint(x) }",
        ),
        (
            "else-if",
            "fn main() { if false { print(1) } else if true { print(2) } else { print(3) } }",
        ),
        (
            "range",
            "fn main() { var n = 0\nfor i in 0..3 { n += i }\nprint(n) }",
        ),
        (
            "string-result",
            "fn value() -> string { return \"ok\" }\nfn main() { print(value()) }",
        ),
        (
            "bool-result",
            "fn yes() -> bool { return true }\nfn main() { print(yes()) }",
        ),
    ];
    for (name, source) in cases {
        analyze(name, source).unwrap_or_else(|errors| panic!("{name}: {}", errors[0]));
    }
}

#[test]
fn rejects_programs_outside_the_supported_language() {
    let cases = [
        (
            "missing-main",
            "fn other() {}",
            "requires a `main` function",
        ),
        (
            "duplicate-function",
            "fn main() {}\nfn main() {}",
            "duplicate function `main`",
        ),
        (
            "duplicate-parameter",
            "fn f(x: int, x: int) {}\nfn main() {}",
            "duplicate parameter `x`",
        ),
        (
            "unknown-local",
            "fn main() { print(x) }",
            "unknown local `x`",
        ),
        (
            "immutable",
            "fn main() { let x = 1\nx = 2 }",
            "cannot assign to immutable `x`",
        ),
        (
            "wrong-assignment",
            "fn main() { var x = 1\nx = true }",
            "expected `int`, found `bool`",
        ),
        (
            "bad-compound",
            "fn main() { var x = true\nx += false }",
            "compound assignment requires `int`",
        ),
        (
            "bad-condition",
            "fn main() { if 1 { print(1) } }",
            "expected `bool`, found `int`",
        ),
        (
            "bad-range",
            "fn main() { for i in false..true { print(i) } }",
            "expected `int`, found `bool`",
        ),
        (
            "unknown-call",
            "fn main() { missing() }",
            "unknown function `missing`",
        ),
        (
            "bad-arity",
            "fn f(x: int) {}\nfn main() { f() }",
            "expects 1 arguments, found 0",
        ),
        (
            "bad-return",
            "fn f() -> int { return true }\nfn main() {}",
            "expected `int`, found `bool`",
        ),
        (
            "missing-return",
            "fn f() -> int { if true { return 1 } }\nfn main() {}",
            "may exit without returning",
        ),
        (
            "string-equality",
            "fn main() { print(\"a\" == \"a\") }",
            "equality supports only `int` and `bool`",
        ),
        ("unused-value", "fn main() { 1 + 2 }", "unused value"),
        (
            "bind-unit",
            "fn main() { let x = print(1) }",
            "cannot bind a no-value expression",
        ),
        (
            "bad-separator",
            "fn main() { let x = 1 let y = 2 }",
            "expected newline or `;`",
        ),
        (
            "unterminated-string",
            "fn main() { print(\"bad) }",
            "unterminated string literal",
        ),
    ];
    for (name, source, expected) in cases {
        let errors = analyze(name, source).expect_err(name);
        assert!(
            errors[0].message.contains(expected),
            "{name}: {}",
            errors[0]
        );
    }
}

#[test]
fn reference_backend_matches_v0_runtime_semantics() {
    let cases = [
        (
            "print",
            "fn main() { print(\"hello\", 7, true, false) }",
            "hello 7 true false\n",
        ),
        (
            "precedence",
            "fn main() { print(1 + 2 * 3, (1 + 2) * 3) }",
            "7 9\n",
        ),
        ("signed", "fn main() { print(-7 / 3, -7 % 3) }", "-2 -1\n"),
        (
            "wrapping",
            "fn main() { print(9223372036854775807 + 1) }",
            "-9223372036854775808\n",
        ),
        (
            "range",
            "fn main() { var sum = 0\nfor i in 1..5 { sum += i }\nprint(sum) }",
            "10\n",
        ),
        (
            "descending",
            "fn main() { for i in 5..1 { print(i) }\nprint(\"done\") }",
            "done\n",
        ),
        (
            "shadow",
            "fn main() { let x = 1\nif true { let x = 2\nprint(x) }\nprint(x) }",
            "2\n1\n",
        ),
        (
            "calls",
            "fn twice(x: int) -> int { return x * 2 }\nfn main() { print(twice(6)) }",
            "12\n",
        ),
        (
            "strings",
            "fn identity(x: string) -> string { return x }\nfn main() { print(identity(\"héllo\")) }",
            "héllo\n",
        ),
        (
            "short-circuit",
            "fn side() -> bool { print(\"evaluated\")\nreturn true }\nfn main() { print(false && side())\nprint(true || side()) }",
            "false\ntrue\n",
        ),
        (
            "argument-order",
            "fn side() -> int { print(\"side\")\nreturn 2 }\nfn main() { print(1, side()) }",
            "side\n1 2\n",
        ),
    ];
    for (name, source, expected) in cases {
        let output = compile_and_run(name, source);
        assert!(
            output.status.success(),
            "{name} exited with {}",
            output.status
        );
        assert_eq!(stdout(&output), expected, "{name}");
    }
}

#[test]
fn main_integer_result_becomes_process_status() {
    let output = compile_and_run("exit-status", "fn main() -> int { return 7 }");
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn invalid_division_traps_instead_of_invoking_undefined_behavior() {
    for (name, expression) in [
        ("zero", "1 / 0"),
        ("overflow", "(-9223372036854775807 - 1) / -1"),
    ] {
        let source = format!("fn main() {{ print({expression}) }}");
        let output = compile_and_run(name, &source);
        assert!(!output.status.success(), "{name} unexpectedly succeeded");
    }
}
