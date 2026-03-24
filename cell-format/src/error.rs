use std::fmt;

/// A position in the source text, tracked as 1-based line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// Errors produced while lexing or parsing a Cellfile.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("{span}: unexpected character '{ch}'")]
    UnexpectedChar { ch: char, span: Span },

    #[error("{span}: unterminated string literal")]
    UnterminatedString { span: Span },

    #[error("{span}: expected {expected}, found {found}")]
    Expected {
        expected: String,
        found: String,
        span: Span,
    },

    #[error("{span}: unknown block '{name}'")]
    UnknownBlock { name: String, span: Span },

    #[error("{span}: invalid integer '{text}'")]
    InvalidInt { text: String, span: Span },

    #[error("{span}: invalid memory value '{text}'")]
    InvalidMemory { text: String, span: Span },

    #[error("{span}: duplicate field '{name}'")]
    DuplicateField { name: String, span: Span },
}

impl ParseError {
    pub fn span(&self) -> Span {
        match self {
            Self::UnexpectedChar { span, .. }
            | Self::UnterminatedString { span, .. }
            | Self::Expected { span, .. }
            | Self::UnknownBlock { span, .. }
            | Self::InvalidInt { span, .. }
            | Self::InvalidMemory { span, .. }
            | Self::DuplicateField { span, .. } => *span,
        }
    }
}
