//! Deterministic frontend and trusted reference backend for Rusty v0.

pub mod ast;
pub mod diagnostic;
pub mod hir;
pub mod lexer;
pub mod llvm;
pub mod parser;
pub mod project;
pub mod rir;
pub mod sema;
pub mod token;

use diagnostic::Diagnostic;
use hir::Program as HirProgram;

pub fn analyze(source_name: &str, source: &str) -> Result<HirProgram, Vec<Diagnostic>> {
    let tokens = lexer::lex(source_name, source)?;
    let ast = parser::parse(source_name, source, tokens)?;
    sema::check(source_name, source, ast)
}

pub fn compile(source_name: &str, source: &str) -> Result<(String, String), Vec<Diagnostic>> {
    let hir = analyze(source_name, source)?;
    let rir = rir::Program::from_hir(&hir);
    Ok((rir.serialize(), llvm::emit(&rir)))
}
