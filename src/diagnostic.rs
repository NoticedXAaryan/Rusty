use crate::token::Span;
use std::fmt::{self, Display, Formatter};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub source_name: String,
    pub message: String,
    pub span: Span,
    pub line: usize,
    pub column: usize,
    pub line_text: String,
}

impl Diagnostic {
    pub fn new(source_name: &str, source: &str, span: Span, message: impl Into<String>) -> Self {
        let offset = span.start.min(source.len());
        let before = &source[..offset];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let line_end = source[offset..]
            .find('\n')
            .map_or(source.len(), |index| offset + index);
        let column = source[line_start..offset].chars().count() + 1;
        Self {
            source_name: source_name.to_owned(),
            message: message.into(),
            span,
            line,
            column,
            line_text: source[line_start..line_end].to_owned(),
        }
    }
}

impl Display for Diagnostic {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "{}:{}:{}: error: {}",
            self.source_name, self.line, self.column, self.message
        )?;
        writeln!(formatter, "  |")?;
        writeln!(formatter, "{:>2} | {}", self.line, self.line_text)?;
        let width = self.span.end.saturating_sub(self.span.start).max(1);
        writeln!(
            formatter,
            "  | {}{}",
            " ".repeat(self.column.saturating_sub(1)),
            "^".repeat(width.min(32))
        )
    }
}

impl std::error::Error for Diagnostic {}
