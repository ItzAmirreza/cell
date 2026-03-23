use crate::error::{ParseError, Span};

/// Token types produced by the Cellfile lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// A bareword identifier (e.g. `cell`, `name`, `env`)
    Ident(String),
    /// A quoted string literal (e.g. `"hello"`)
    StringLit(String),
    /// An integer literal (e.g. `8080`)
    IntLit(i64),
    /// `=`
    Equals,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `,`
    Comma,
    /// End of file
    Eof,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Ident(s) => write!(f, "{s}"),
            Token::StringLit(s) => write!(f, "\"{s}\""),
            Token::IntLit(n) => write!(f, "{n}"),
            Token::Equals => write!(f, "="),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::Comma => write!(f, ","),
            Token::Eof => write!(f, "EOF"),
        }
    }
}

/// Tokenize a Cellfile source string into a list of (Token, Span) pairs.
pub fn tokenize(input: &str) -> Result<Vec<(Token, Span)>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();
    let mut line = 1usize;
    let mut line_start = 0usize;

    while let Some(&(i, ch)) = chars.peek() {
        let col = i - line_start + 1;

        match ch {
            // Whitespace
            ' ' | '\t' | '\r' => {
                chars.next();
            }
            '\n' => {
                chars.next();
                line += 1;
                line_start = i + 1;
            }
            // Comments
            '#' => {
                chars.next();
                while let Some(&(_, c)) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            // Single-char tokens
            '=' => {
                tokens.push((Token::Equals, Span { line, col }));
                chars.next();
            }
            '{' => {
                tokens.push((Token::LBrace, Span { line, col }));
                chars.next();
            }
            '}' => {
                tokens.push((Token::RBrace, Span { line, col }));
                chars.next();
            }
            '[' => {
                tokens.push((Token::LBracket, Span { line, col }));
                chars.next();
            }
            ']' => {
                tokens.push((Token::RBracket, Span { line, col }));
                chars.next();
            }
            ',' => {
                tokens.push((Token::Comma, Span { line, col }));
                chars.next();
            }
            // String literals
            '"' => {
                let span = Span { line, col };
                chars.next(); // consume opening quote
                let mut s = String::new();
                let mut closed = false;
                while let Some(&(_, c)) = chars.peek() {
                    chars.next();
                    if c == '\\' {
                        // Escape sequences
                        if let Some(&(_, esc)) = chars.peek() {
                            chars.next();
                            match esc {
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                '\\' => s.push('\\'),
                                '"' => s.push('"'),
                                other => {
                                    s.push('\\');
                                    s.push(other);
                                }
                            }
                        }
                    } else if c == '"' {
                        closed = true;
                        break;
                    } else {
                        s.push(c);
                    }
                }
                if !closed {
                    return Err(ParseError::UnterminatedString { span });
                }
                tokens.push((Token::StringLit(s), span));
            }
            // Integers
            c if c.is_ascii_digit() => {
                let span = Span { line, col };
                let mut num = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_digit() {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value: i64 = num.parse().map_err(|_| ParseError::InvalidInt {
                    value: num,
                    span: span.clone(),
                })?;
                tokens.push((Token::IntLit(value), span));
            }
            // Identifiers and keywords
            c if c.is_ascii_alphabetic() || c == '_' => {
                let span = Span { line, col };
                let mut ident = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        ident.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push((Token::Ident(ident), span));
            }
            _ => {
                return Err(ParseError::UnexpectedChar {
                    ch,
                    span: Span { line, col },
                });
            }
        }
    }

    tokens.push((Token::Eof, Span { line, col: 1 }));
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_tokens() {
        let tokens = tokenize("cell { }").unwrap();
        assert_eq!(tokens[0].0, Token::Ident("cell".into()));
        assert_eq!(tokens[1].0, Token::LBrace);
        assert_eq!(tokens[2].0, Token::RBrace);
        assert_eq!(tokens[3].0, Token::Eof);
    }

    #[test]
    fn test_string_literal() {
        let tokens = tokenize(r#"name = "hello""#).unwrap();
        assert_eq!(tokens[0].0, Token::Ident("name".into()));
        assert_eq!(tokens[1].0, Token::Equals);
        assert_eq!(tokens[2].0, Token::StringLit("hello".into()));
    }

    #[test]
    fn test_integer_array() {
        let tokens = tokenize("[8080, 3000]").unwrap();
        assert_eq!(tokens[0].0, Token::LBracket);
        assert_eq!(tokens[1].0, Token::IntLit(8080));
        assert_eq!(tokens[2].0, Token::Comma);
        assert_eq!(tokens[3].0, Token::IntLit(3000));
        assert_eq!(tokens[4].0, Token::RBracket);
    }

    #[test]
    fn test_comments_stripped() {
        let tokens = tokenize("# this is a comment\ncell").unwrap();
        assert_eq!(tokens[0].0, Token::Ident("cell".into()));
        assert_eq!(tokens[1].0, Token::Eof);
    }

    #[test]
    fn test_escape_sequences() {
        let tokens = tokenize(r#""hello\nworld""#).unwrap();
        assert_eq!(tokens[0].0, Token::StringLit("hello\nworld".into()));
    }

    #[test]
    fn test_unterminated_string() {
        let result = tokenize(r#""hello"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_span_tracking() {
        let tokens = tokenize("cell {\n  name\n}").unwrap();
        assert_eq!(tokens[0].1, Span { line: 1, col: 1 });
        assert_eq!(tokens[2].1, Span { line: 2, col: 3 }); // "name"
        assert_eq!(tokens[3].1, Span { line: 3, col: 1 }); // "}"
    }

    #[test]
    fn test_hyphenated_ident() {
        let tokens = tokenize("NODE_ENV").unwrap();
        assert_eq!(tokens[0].0, Token::Ident("NODE_ENV".into()));
    }
}
