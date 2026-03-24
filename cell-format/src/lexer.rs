use crate::error::{ParseError, Span};

/// Tokens produced by the Cellfile lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A bare identifier such as `cell`, `name`, `env`, `to`, …
    Ident(String),
    /// A double-quoted string literal (escape sequences resolved).
    StringLit(String),
    /// An integer literal.
    IntLit(i64),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Equals,
    Comma,
    Eof,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Ident(s) => write!(f, "identifier `{s}`"),
            Token::StringLit(s) => write!(f, "string \"{s}\""),
            Token::IntLit(n) => write!(f, "integer {n}"),
            Token::LBrace => write!(f, "`{{`"),
            Token::RBrace => write!(f, "`}}`"),
            Token::LBracket => write!(f, "`[`"),
            Token::RBracket => write!(f, "`]`"),
            Token::Equals => write!(f, "`=`"),
            Token::Comma => write!(f, "`,`"),
            Token::Eof => write!(f, "end of input"),
        }
    }
}

/// Tokenize a Cellfile source string into a sequence of (Token, Span) pairs.
///
/// Comments (lines starting with `#`) are stripped.  The final token is always
/// `Token::Eof`.
pub fn tokenize(input: &str) -> Result<Vec<(Token, Span)>, ParseError> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos: usize = 0;
    let mut line: usize = 1;
    let mut col: usize = 1;

    while pos < len {
        let b = bytes[pos];

        // --- whitespace ---
        if b == b' ' || b == b'\t' || b == b'\r' {
            pos += 1;
            col += 1;
            continue;
        }
        if b == b'\n' {
            pos += 1;
            line += 1;
            col = 1;
            continue;
        }

        // --- comment ---
        if b == b'#' {
            while pos < len && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }

        // --- single-character tokens ---
        let span = Span::new(line, col);
        match b {
            b'{' => {
                tokens.push((Token::LBrace, span));
                pos += 1;
                col += 1;
                continue;
            }
            b'}' => {
                tokens.push((Token::RBrace, span));
                pos += 1;
                col += 1;
                continue;
            }
            b'[' => {
                tokens.push((Token::LBracket, span));
                pos += 1;
                col += 1;
                continue;
            }
            b']' => {
                tokens.push((Token::RBracket, span));
                pos += 1;
                col += 1;
                continue;
            }
            b'=' => {
                tokens.push((Token::Equals, span));
                pos += 1;
                col += 1;
                continue;
            }
            b',' => {
                tokens.push((Token::Comma, span));
                pos += 1;
                col += 1;
                continue;
            }
            _ => {}
        }

        // --- string literal ---
        if b == b'"' {
            let start_span = Span::new(line, col);
            pos += 1;
            col += 1;
            let mut value = String::new();
            loop {
                if pos >= len {
                    return Err(ParseError::UnterminatedString { span: start_span });
                }
                let c = bytes[pos];
                if c == b'"' {
                    pos += 1;
                    col += 1;
                    break;
                }
                if c == b'\\' {
                    pos += 1;
                    col += 1;
                    if pos >= len {
                        return Err(ParseError::UnterminatedString { span: start_span });
                    }
                    let esc = bytes[pos];
                    let resolved = match esc {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'\\' => '\\',
                        b'"' => '"',
                        _ => {
                            // Keep the literal character after the backslash.
                            esc as char
                        }
                    };
                    value.push(resolved);
                    pos += 1;
                    col += 1;
                    continue;
                }
                if c == b'\n' {
                    // Newline inside a string — allow it but track position.
                    value.push('\n');
                    pos += 1;
                    line += 1;
                    col = 1;
                    continue;
                }
                value.push(c as char);
                pos += 1;
                col += 1;
            }
            tokens.push((Token::StringLit(value), start_span));
            continue;
        }

        // --- integer literal ---
        if b.is_ascii_digit() {
            let start_col = col;
            let start_pos = pos;
            while pos < len && bytes[pos].is_ascii_digit() {
                pos += 1;
                col += 1;
            }
            let text = &input[start_pos..pos];
            let n: i64 = text.parse().map_err(|_| ParseError::InvalidInt {
                text: text.to_owned(),
                span: Span::new(line, start_col),
            })?;
            tokens.push((Token::IntLit(n), Span::new(line, start_col)));
            continue;
        }

        // --- identifier / bareword ---
        if b.is_ascii_alphabetic() || b == b'_' {
            let start_col = col;
            let start_pos = pos;
            while pos < len
                && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_' || bytes[pos] == b'-')
            {
                pos += 1;
                col += 1;
            }
            let word = input[start_pos..pos].to_owned();
            tokens.push((Token::Ident(word), Span::new(line, start_col)));
            continue;
        }

        // --- unknown ---
        return Err(ParseError::UnexpectedChar {
            ch: b as char,
            span: Span::new(line, col),
        });
    }

    tokens.push((Token::Eof, Span::new(line, col)));
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let tokens = tokenize("").unwrap();
        assert_eq!(tokens, vec![(Token::Eof, Span::new(1, 1))]);
    }

    #[test]
    fn single_braces() {
        let tokens = tokenize("{ }").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::LBrace, Span::new(1, 1)),
                (Token::RBrace, Span::new(1, 3)),
                (Token::Eof, Span::new(1, 4)),
            ]
        );
    }

    #[test]
    fn identifiers_and_equals() {
        let tokens = tokenize("name = foo").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::Ident("name".into()), Span::new(1, 1)),
                (Token::Equals, Span::new(1, 6)),
                (Token::Ident("foo".into()), Span::new(1, 8)),
                (Token::Eof, Span::new(1, 11)),
            ]
        );
    }

    #[test]
    fn string_literal_with_escapes() {
        let tokens = tokenize(r#""hello\nworld""#).unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::StringLit("hello\nworld".into()), Span::new(1, 1)),
                (Token::Eof, Span::new(1, 15)),
            ]
        );
    }

    #[test]
    fn integer_literal() {
        let tokens = tokenize("8080").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::IntLit(8080), Span::new(1, 1)),
                (Token::Eof, Span::new(1, 5)),
            ]
        );
    }

    #[test]
    fn comment_is_skipped() {
        let tokens = tokenize("# this is a comment\nfoo").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::Ident("foo".into()), Span::new(2, 1)),
                (Token::Eof, Span::new(2, 4)),
            ]
        );
    }

    #[test]
    fn brackets_and_commas() {
        let tokens = tokenize("[8080, 443]").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::LBracket, Span::new(1, 1)),
                (Token::IntLit(8080), Span::new(1, 2)),
                (Token::Comma, Span::new(1, 6)),
                (Token::IntLit(443), Span::new(1, 8)),
                (Token::RBracket, Span::new(1, 11)),
                (Token::Eof, Span::new(1, 12)),
            ]
        );
    }

    #[test]
    fn unterminated_string() {
        let err = tokenize("\"oops").unwrap_err();
        assert!(matches!(err, ParseError::UnterminatedString { .. }));
    }

    #[test]
    fn unexpected_char() {
        let err = tokenize("@").unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedChar { ch: '@', .. }));
    }

    #[test]
    fn multiline_tracking() {
        let tokens = tokenize("a\nb").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::Ident("a".into()), Span::new(1, 1)),
                (Token::Ident("b".into()), Span::new(2, 1)),
                (Token::Eof, Span::new(2, 2)),
            ]
        );
    }

    #[test]
    fn hyphenated_ident() {
        let tokens = tokenize("my-app").unwrap();
        assert_eq!(
            tokens,
            vec![
                (Token::Ident("my-app".into()), Span::new(1, 1)),
                (Token::Eof, Span::new(1, 7)),
            ]
        );
    }
}
