use rusty_lang::{analyze, compile};

#[test]
fn compiles_complete_v0_control_flow() {
    let source = r#"
fn classify(value: int) -> bool {
    return value >= 10 && value != 12
}
fn main() {
    var total = 0
    for i in 0..5 {
        total += i * 2
    }
    if classify(total) {
        print("total", total, true)
    } else {
        print("small")
    }
}
"#;
    let (rir, llvm) = compile("conformance.ry", source).expect("valid program");
    assert!(rir.contains("add-set"));
    assert!(rir.contains("(for @"));
    assert!(llvm.contains("phi i1"));
    assert!(llvm.contains("icmp slt"));
}

#[test]
fn deterministic_outputs_are_byte_identical() {
    let source = "fn main() { print(\"stable\", 1 + 2) }";
    assert_eq!(
        compile("a.ry", source).unwrap(),
        compile("different-name.ry", source).unwrap()
    );
}

#[test]
fn diagnostics_include_source_location() {
    let errors = analyze("bad.ry", "fn main() { var x = true\nx += 1 }").unwrap_err();
    let rendered = errors[0].to_string();
    assert!(rendered.contains("bad.ry:2:1"));
    assert!(rendered.contains("compound assignment requires `int`"));
}
