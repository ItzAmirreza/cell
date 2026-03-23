/// Span tracks the location of a token in source text.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}, col {}", self.line, self.col)
    }
}

/// Errors produced by the Cellfile lexer and parser.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected character '{ch}' at {span}")]
    UnexpectedChar { ch: char, span: Span },

    #[error("unterminated string at {span}")]
    UnterminatedString { span: Span },

    #[error("unexpected token '{found}' at {span}, expected {expected}")]
    UnexpectedToken {
        found: String,
        span: Span,
        expected: String,
    },

    #[error("unexpected end of input, expected {expected}")]
    UnexpectedEof { expected: String },

    #[error("duplicate field '{field}' at {span}")]
    DuplicateField { field: String, span: Span },

    #[error("missing required field '{field}'")]
    MissingField { field: String },

    #[error("invalid integer '{value}' at {span}")]
    InvalidInt { value: String, span: Span },
}
