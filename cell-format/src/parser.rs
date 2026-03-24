use crate::ast::{CellSpec, EnvVar, FsOp, ResourceLimits};
use crate::error::{ParseError, Span};
use crate::lexer::{tokenize, Token};

/// Recursive-descent parser for the Cellfile format.
///
/// ```text
/// cell {
///   name = "myapp"
///   base = "alpine:3.19"
///   env { KEY = "value" }
///   fs { copy "src/" to "/app/src" }
///   run = "/app/start.sh"
///   expose = [8080]
///   limits { memory = "512MB" processes = 10 }
/// }
/// ```
pub struct Parser {
    tokens: Vec<(Token, Span)>,
    pos: usize,
}

impl Parser {
    /// Parse a Cellfile source string into a [`CellSpec`].
    pub fn parse(input: &str) -> Result<CellSpec, ParseError> {
        let tokens = tokenize(input)?;
        let mut p = Parser { tokens, pos: 0 };
        p.parse_cell()
    }

    // ---- helpers ----------------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos].0
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].1
    }

    fn advance(&mut self) -> (Token, Span) {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), ParseError> {
        match self.peek().clone() {
            Token::Ident(ref s) => {
                let s = s.clone();
                let span = self.span();
                self.advance();
                Ok((s, span))
            }
            other => Err(ParseError::Expected {
                expected: what.to_owned(),
                found: other.to_string(),
                span: self.span(),
            }),
        }
    }

    fn expect_token(&mut self, expected: &Token) -> Result<Span, ParseError> {
        if self.peek() == expected {
            let span = self.span();
            self.advance();
            Ok(span)
        } else {
            Err(ParseError::Expected {
                expected: expected.to_string(),
                found: self.peek().to_string(),
                span: self.span(),
            })
        }
    }

    fn expect_string(&mut self, what: &str) -> Result<(String, Span), ParseError> {
        match self.peek().clone() {
            Token::StringLit(ref s) => {
                let s = s.clone();
                let span = self.span();
                self.advance();
                Ok((s, span))
            }
            other => Err(ParseError::Expected {
                expected: what.to_owned(),
                found: other.to_string(),
                span: self.span(),
            }),
        }
    }

    fn expect_int(&mut self, what: &str) -> Result<(i64, Span), ParseError> {
        match self.peek().clone() {
            Token::IntLit(n) => {
                let span = self.span();
                self.advance();
                Ok((n, span))
            }
            other => Err(ParseError::Expected {
                expected: what.to_owned(),
                found: other.to_string(),
                span: self.span(),
            }),
        }
    }

    // ---- grammar ----------------------------------------------------------

    fn parse_cell(&mut self) -> Result<CellSpec, ParseError> {
        // "cell" "{"
        let (kw, kw_span) = self.expect_ident("keyword `cell`")?;
        if kw != "cell" {
            return Err(ParseError::Expected {
                expected: "keyword `cell`".to_owned(),
                found: format!("identifier `{kw}`"),
                span: kw_span,
            });
        }
        self.expect_token(&Token::LBrace)?;

        let mut name: Option<String> = None;
        let mut base: Option<String> = None;
        let mut env: Vec<EnvVar> = Vec::new();
        let mut fs_ops: Vec<FsOp> = Vec::new();
        let mut run: Option<String> = None;
        let mut expose: Vec<u16> = Vec::new();
        let mut limits: Option<ResourceLimits> = None;

        while *self.peek() != Token::RBrace {
            let (field, field_span) = self.expect_ident("field name")?;
            match field.as_str() {
                "name" => {
                    if name.is_some() {
                        return Err(ParseError::DuplicateField {
                            name: "name".into(),
                            span: field_span,
                        });
                    }
                    self.expect_token(&Token::Equals)?;
                    let (v, _) = self.expect_string("string value for `name`")?;
                    name = Some(v);
                }
                "base" => {
                    if base.is_some() {
                        return Err(ParseError::DuplicateField {
                            name: "base".into(),
                            span: field_span,
                        });
                    }
                    self.expect_token(&Token::Equals)?;
                    let (v, _) = self.expect_string("string value for `base`")?;
                    base = Some(v);
                }
                "run" => {
                    if run.is_some() {
                        return Err(ParseError::DuplicateField {
                            name: "run".into(),
                            span: field_span,
                        });
                    }
                    self.expect_token(&Token::Equals)?;
                    let (v, _) = self.expect_string("string value for `run`")?;
                    run = Some(v);
                }
                "env" => {
                    self.parse_env_block(&mut env)?;
                }
                "fs" => {
                    self.parse_fs_block(&mut fs_ops)?;
                }
                "expose" => {
                    self.expect_token(&Token::Equals)?;
                    self.parse_expose_list(&mut expose)?;
                }
                "limits" => {
                    if limits.is_some() {
                        return Err(ParseError::DuplicateField {
                            name: "limits".into(),
                            span: field_span,
                        });
                    }
                    limits = Some(self.parse_limits_block()?);
                }
                other => {
                    return Err(ParseError::UnknownBlock {
                        name: other.to_owned(),
                        span: field_span,
                    });
                }
            }
        }

        // closing "}"
        self.expect_token(&Token::RBrace)?;

        let name = name.ok_or_else(|| ParseError::Expected {
            expected: "field `name`".to_owned(),
            found: "end of block".to_owned(),
            span: self.span(),
        })?;
        let base = base.ok_or_else(|| ParseError::Expected {
            expected: "field `base`".to_owned(),
            found: "end of block".to_owned(),
            span: self.span(),
        })?;

        Ok(CellSpec {
            name,
            base,
            env,
            fs_ops,
            run,
            expose,
            limits,
        })
    }

    /// ```text
    /// env { KEY = "value" ... }
    /// ```
    fn parse_env_block(&mut self, env: &mut Vec<EnvVar>) -> Result<(), ParseError> {
        self.expect_token(&Token::LBrace)?;
        while *self.peek() != Token::RBrace {
            let (key, _) = self.expect_ident("environment variable name")?;
            self.expect_token(&Token::Equals)?;
            let (value, _) = self.expect_string("string value")?;
            env.push(EnvVar { key, value });
        }
        self.expect_token(&Token::RBrace)?;
        Ok(())
    }

    /// ```text
    /// fs { copy "src" to "dest" ... }
    /// ```
    fn parse_fs_block(&mut self, fs_ops: &mut Vec<FsOp>) -> Result<(), ParseError> {
        self.expect_token(&Token::LBrace)?;
        while *self.peek() != Token::RBrace {
            let (op, op_span) = self.expect_ident("fs operation")?;
            match op.as_str() {
                "copy" => {
                    let (src, _) = self.expect_string("source path")?;
                    let (kw, kw_span) = self.expect_ident("keyword `to`")?;
                    if kw != "to" {
                        return Err(ParseError::Expected {
                            expected: "keyword `to`".to_owned(),
                            found: format!("identifier `{kw}`"),
                            span: kw_span,
                        });
                    }
                    let (dest, _) = self.expect_string("destination path")?;
                    fs_ops.push(FsOp::Copy { src, dest });
                }
                other => {
                    return Err(ParseError::UnknownBlock {
                        name: other.to_owned(),
                        span: op_span,
                    });
                }
            }
        }
        self.expect_token(&Token::RBrace)?;
        Ok(())
    }

    /// ```text
    /// expose = [8080, 443]
    /// ```
    fn parse_expose_list(&mut self, expose: &mut Vec<u16>) -> Result<(), ParseError> {
        self.expect_token(&Token::LBracket)?;
        if *self.peek() != Token::RBracket {
            loop {
                let (n, span) = self.expect_int("port number")?;
                let port =
                    u16::try_from(n).map_err(|_| ParseError::InvalidInt {
                        text: n.to_string(),
                        span,
                    })?;
                expose.push(port);
                if *self.peek() == Token::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        self.expect_token(&Token::RBracket)?;
        Ok(())
    }

    /// ```text
    /// limits { memory = "512MB" processes = 10 }
    /// ```
    fn parse_limits_block(&mut self) -> Result<ResourceLimits, ParseError> {
        self.expect_token(&Token::LBrace)?;
        let mut memory: Option<u64> = None;
        let mut processes: Option<u64> = None;

        while *self.peek() != Token::RBrace {
            let (key, key_span) = self.expect_ident("limits field")?;
            self.expect_token(&Token::Equals)?;
            match key.as_str() {
                "memory" => {
                    if memory.is_some() {
                        return Err(ParseError::DuplicateField {
                            name: "memory".into(),
                            span: key_span,
                        });
                    }
                    let (raw, raw_span) = self.expect_string("memory value")?;
                    memory = Some(parse_memory(&raw, raw_span)?);
                }
                "processes" => {
                    if processes.is_some() {
                        return Err(ParseError::DuplicateField {
                            name: "processes".into(),
                            span: key_span,
                        });
                    }
                    let (n, span) = self.expect_int("integer")?;
                    processes = Some(u64::try_from(n).map_err(|_| ParseError::InvalidInt {
                        text: n.to_string(),
                        span,
                    })?);
                }
                other => {
                    return Err(ParseError::UnknownBlock {
                        name: other.to_owned(),
                        span: key_span,
                    });
                }
            }
        }
        self.expect_token(&Token::RBrace)?;

        Ok(ResourceLimits { memory, processes })
    }
}

/// Parse a human-readable memory string such as `"512MB"`, `"1GB"`, `"2048KB"`
/// into bytes.
fn parse_memory(raw: &str, span: Span) -> Result<u64, ParseError> {
    let s = raw.trim();
    let make_err = || ParseError::InvalidMemory {
        text: raw.to_owned(),
        span,
    };

    // Find where the numeric part ends.
    let num_end = s
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(s.len());
    if num_end == 0 {
        return Err(make_err());
    }

    let number: u64 = s[..num_end].parse().map_err(|_| make_err())?;
    let suffix = s[num_end..].trim().to_ascii_uppercase();

    let multiplier: u64 = match suffix.as_str() {
        "" | "B" => 1,
        "KB" | "K" => 1024,
        "MB" | "M" => 1024 * 1024,
        "GB" | "G" => 1024 * 1024 * 1024,
        "TB" | "T" => 1024 * 1024 * 1024 * 1024,
        _ => return Err(make_err()),
    };

    number.checked_mul(multiplier).ok_or_else(make_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{CellSpec, EnvVar, FsOp, ResourceLimits};

    fn parse(input: &str) -> CellSpec {
        Parser::parse(input).expect("parse failed")
    }

    #[test]
    fn minimal_cell() {
        let spec = parse(
            r#"
            cell {
                name = "myapp"
                base = "alpine:3.19"
            }
            "#,
        );
        assert_eq!(spec.name, "myapp");
        assert_eq!(spec.base, "alpine:3.19");
        assert!(spec.env.is_empty());
        assert!(spec.fs_ops.is_empty());
        assert!(spec.run.is_none());
        assert!(spec.expose.is_empty());
        assert!(spec.limits.is_none());
    }

    #[test]
    fn full_cell() {
        let spec = parse(
            r#"
            cell {
                name = "myapp"
                base = "alpine:3.19"
                env {
                    KEY = "value"
                    OTHER = "stuff"
                }
                fs {
                    copy "src/" to "/app/src"
                }
                run = "/app/start.sh"
                expose = [8080]
                limits {
                    memory = "512MB"
                    processes = 10
                }
            }
            "#,
        );
        assert_eq!(spec.name, "myapp");
        assert_eq!(spec.base, "alpine:3.19");
        assert_eq!(
            spec.env,
            vec![
                EnvVar {
                    key: "KEY".into(),
                    value: "value".into(),
                },
                EnvVar {
                    key: "OTHER".into(),
                    value: "stuff".into(),
                },
            ]
        );
        assert_eq!(
            spec.fs_ops,
            vec![FsOp::Copy {
                src: "src/".into(),
                dest: "/app/src".into(),
            }]
        );
        assert_eq!(spec.run.as_deref(), Some("/app/start.sh"));
        assert_eq!(spec.expose, vec![8080]);
        assert_eq!(
            spec.limits,
            Some(ResourceLimits {
                memory: Some(512 * 1024 * 1024),
                processes: Some(10),
            })
        );
    }

    #[test]
    fn multiple_expose_ports() {
        let spec = parse(
            r#"
            cell {
                name = "web"
                base = "nginx"
                expose = [80, 443, 8080]
            }
            "#,
        );
        assert_eq!(spec.expose, vec![80, 443, 8080]);
    }

    #[test]
    fn multiple_fs_ops() {
        let spec = parse(
            r#"
            cell {
                name = "app"
                base = "debian"
                fs {
                    copy "a" to "/a"
                    copy "b" to "/b"
                }
            }
            "#,
        );
        assert_eq!(spec.fs_ops.len(), 2);
    }

    #[test]
    fn comments_are_ignored() {
        let spec = parse(
            r#"
            # top comment
            cell {
                name = "app" # inline comment
                base = "alpine"
            }
            "#,
        );
        assert_eq!(spec.name, "app");
    }

    #[test]
    fn limits_memory_only() {
        let spec = parse(
            r#"
            cell {
                name = "x"
                base = "y"
                limits {
                    memory = "1GB"
                }
            }
            "#,
        );
        assert_eq!(
            spec.limits,
            Some(ResourceLimits {
                memory: Some(1024 * 1024 * 1024),
                processes: None,
            })
        );
    }

    #[test]
    fn limits_processes_only() {
        let spec = parse(
            r#"
            cell {
                name = "x"
                base = "y"
                limits {
                    processes = 42
                }
            }
            "#,
        );
        assert_eq!(
            spec.limits,
            Some(ResourceLimits {
                memory: None,
                processes: Some(42),
            })
        );
    }

    #[test]
    fn error_missing_name() {
        let err = Parser::parse(
            r#"
            cell {
                base = "alpine"
            }
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::Expected { .. }));
    }

    #[test]
    fn error_unknown_field() {
        let err = Parser::parse(
            r#"
            cell {
                name = "x"
                base = "y"
                bogus = "z"
            }
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::UnknownBlock { .. }));
    }

    #[test]
    fn error_duplicate_name() {
        let err = Parser::parse(
            r#"
            cell {
                name = "a"
                name = "b"
                base = "c"
            }
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ParseError::DuplicateField { .. }));
    }

    #[test]
    fn parse_memory_values() {
        let span = crate::error::Span::new(1, 1);
        assert_eq!(parse_memory("100", span).unwrap(), 100);
        assert_eq!(parse_memory("100B", span).unwrap(), 100);
        assert_eq!(parse_memory("4KB", span).unwrap(), 4096);
        assert_eq!(parse_memory("512MB", span).unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_memory("2GB", span).unwrap(), 2 * 1024 * 1024 * 1024);
        assert!(parse_memory("", span).is_err());
        assert!(parse_memory("XY", span).is_err());
    }

    #[test]
    fn roundtrip_serde() {
        let spec = parse(
            r#"
            cell {
                name = "serde-test"
                base = "alpine"
                run = "/start"
                expose = [9090]
            }
            "#,
        );
        let json = serde_json::to_string(&spec).expect("serialize");
        let back: CellSpec = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(spec, back);
    }
}
