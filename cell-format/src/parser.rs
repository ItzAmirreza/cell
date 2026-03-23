use crate::ast::{CellSpec, EnvVar, FsOp};
use crate::error::{ParseError, Span};
use crate::lexer::{self, Token};

/// Recursive-descent parser for the Cellfile format.
pub struct Parser {
    tokens: Vec<(Token, Span)>,
    pos: usize,
}

impl Parser {
    /// Parse a Cellfile source string into a `CellSpec`.
    pub fn parse(input: &str) -> Result<CellSpec, ParseError> {
        let tokens = lexer::tokenize(input)?;
        let mut parser = Parser { tokens, pos: 0 };
        parser.parse_cellfile()
    }

    // ── Helpers ──────────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.tokens[self.pos].0
    }

    fn peek_span(&self) -> &Span {
        &self.tokens[self.pos].1
    }

    fn advance(&mut self) -> (Token, Span) {
        let tok = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect_ident(&mut self, expected: &str) -> Result<Span, ParseError> {
        let (tok, span) = self.advance();
        match &tok {
            Token::Ident(s) if s == expected => Ok(span),
            Token::Eof => Err(ParseError::UnexpectedEof {
                expected: format!("'{expected}'"),
            }),
            _ => Err(ParseError::UnexpectedToken {
                found: tok.to_string(),
                span,
                expected: format!("'{expected}'"),
            }),
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<Span, ParseError> {
        let (tok, span) = self.advance();
        if &tok == expected {
            Ok(span)
        } else if tok == Token::Eof {
            Err(ParseError::UnexpectedEof {
                expected: expected.to_string(),
            })
        } else {
            Err(ParseError::UnexpectedToken {
                found: tok.to_string(),
                span,
                expected: expected.to_string(),
            })
        }
    }

    // ── Grammar rules ────────────────────────────────────────

    /// cellfile ::= "cell" "{" statement* "}"
    fn parse_cellfile(&mut self) -> Result<CellSpec, ParseError> {
        self.expect_ident("cell")?;
        self.expect(&Token::LBrace)?;

        let mut name: Option<String> = None;
        let mut base: Option<String> = None;
        let mut env: Vec<EnvVar> = Vec::new();
        let mut fs_ops: Vec<FsOp> = Vec::new();
        let mut run: Option<String> = None;
        let mut expose: Vec<u16> = Vec::new();
        let mut limits: Option<crate::ast::ResourceLimits> = None;
        let mut ports: Vec<crate::ast::PortMapping> = Vec::new();
        let mut volumes: Vec<crate::ast::VolumeMount> = Vec::new();

        loop {
            match self.peek() {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "'}'".into(),
                    });
                }
                Token::Ident(field) => {
                    let field = field.clone();
                    let span = self.peek_span().clone();

                    match field.as_str() {
                        "name" => {
                            if name.is_some() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            self.expect(&Token::Equals)?;
                            name = Some(self.parse_string()?);
                        }
                        "base" => {
                            if base.is_some() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            self.expect(&Token::Equals)?;
                            base = Some(self.parse_string()?);
                        }
                        "run" => {
                            if run.is_some() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            self.expect(&Token::Equals)?;
                            run = Some(self.parse_string()?);
                        }
                        "expose" => {
                            if !expose.is_empty() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            self.expect(&Token::Equals)?;
                            expose = self.parse_int_array()?;
                        }
                        "env" => {
                            if !env.is_empty() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            env = self.parse_env_block()?;
                        }
                        "fs" => {
                            if !fs_ops.is_empty() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            fs_ops = self.parse_fs_block()?;
                        }
                        "limits" => {
                            if limits.is_some() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            limits = Some(self.parse_limits_block()?);
                        }
                        "ports" => {
                            if !ports.is_empty() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            ports = self.parse_ports_block()?;
                        }
                        "volumes" => {
                            if !volumes.is_empty() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            volumes = self.parse_volumes_block()?;
                        }
                        _ => {
                            return Err(ParseError::UnexpectedToken {
                                found: field,
                                span,
                                expected: "a valid field (name, base, env, fs, run, expose, limits, ports, volumes)"
                                    .into(),
                            });
                        }
                    }
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "a field name".into(),
                    });
                }
            }
        }

        let name = name.ok_or(ParseError::MissingField {
            field: "name".into(),
        })?;

        Ok(CellSpec {
            name,
            base,
            env,
            fs_ops,
            run,
            expose,
            limits,
            ports,
            volumes,
        })
    }

    /// Parse a string literal token.
    fn parse_string(&mut self) -> Result<String, ParseError> {
        let (tok, span) = self.advance();
        match tok {
            Token::StringLit(s) => Ok(s),
            Token::Eof => Err(ParseError::UnexpectedEof {
                expected: "string".into(),
            }),
            _ => Err(ParseError::UnexpectedToken {
                found: tok.to_string(),
                span,
                expected: "string".into(),
            }),
        }
    }

    /// Parse an array of integers: `[8080, 3000]`
    fn parse_int_array(&mut self) -> Result<Vec<u16>, ParseError> {
        self.expect(&Token::LBracket)?;
        let mut values = Vec::new();

        loop {
            match self.peek() {
                Token::RBracket => {
                    self.advance();
                    break;
                }
                Token::IntLit(_) => {
                    let (tok, span) = self.advance();
                    if let Token::IntLit(n) = tok {
                        let port = u16::try_from(n).map_err(|_| ParseError::InvalidInt {
                            value: n.to_string(),
                            span: span.clone(),
                        })?;
                        values.push(port);
                    }
                    // Optional trailing comma
                    if *self.peek() == Token::Comma {
                        self.advance();
                    }
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "']'".into(),
                    });
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "integer or ']'".into(),
                    });
                }
            }
        }

        Ok(values)
    }

    /// Parse an env block: `{ KEY = "value" ... }`
    fn parse_env_block(&mut self) -> Result<Vec<EnvVar>, ParseError> {
        self.expect(&Token::LBrace)?;
        let mut vars = Vec::new();

        loop {
            match self.peek() {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::Ident(_) => {
                    let (tok, _) = self.advance();
                    let key = match tok {
                        Token::Ident(s) => s,
                        _ => unreachable!(),
                    };
                    self.expect(&Token::Equals)?;
                    let value = self.parse_string()?;
                    vars.push(EnvVar { key, value });
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "'}'".into(),
                    });
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "environment variable name or '}'".into(),
                    });
                }
            }
        }

        Ok(vars)
    }

    /// Parse a fs block: `{ copy "src" to "dest" ... }`
    fn parse_fs_block(&mut self) -> Result<Vec<FsOp>, ParseError> {
        self.expect(&Token::LBrace)?;
        let mut ops = Vec::new();

        loop {
            match self.peek() {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::Ident(kw) if kw == "copy" => {
                    self.advance();
                    let src = self.parse_string()?;
                    self.expect_ident("to")?;
                    let dest = self.parse_string()?;
                    ops.push(FsOp::Copy { src, dest });
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "'}'".into(),
                    });
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "'copy' or '}'".into(),
                    });
                }
            }
        }

        Ok(ops)
    }

    /// Parse a limits block: `{ memory = "512MB" processes = 10 }`
    fn parse_limits_block(&mut self) -> Result<crate::ast::ResourceLimits, ParseError> {
        self.expect(&Token::LBrace)?;
        let mut memory: Option<String> = None;
        let mut processes: Option<u32> = None;

        loop {
            match self.peek() {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::Ident(field) => {
                    let field = field.clone();
                    let span = self.peek_span().clone();

                    match field.as_str() {
                        "memory" => {
                            if memory.is_some() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            self.expect(&Token::Equals)?;
                            memory = Some(self.parse_string()?);
                        }
                        "processes" => {
                            if processes.is_some() {
                                return Err(ParseError::DuplicateField { field, span });
                            }
                            self.advance();
                            self.expect(&Token::Equals)?;
                            let (tok, span) = self.advance();
                            match tok {
                                Token::IntLit(n) => {
                                    processes = Some(n as u32);
                                }
                                _ => {
                                    return Err(ParseError::UnexpectedToken {
                                        found: tok.to_string(),
                                        span,
                                        expected: "integer".into(),
                                    });
                                }
                            }
                        }
                        _ => {
                            return Err(ParseError::UnexpectedToken {
                                found: field,
                                span,
                                expected: "'memory' or 'processes'".into(),
                            });
                        }
                    }
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "'}'".into(),
                    });
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "limit field or '}'".into(),
                    });
                }
            }
        }

        Ok(crate::ast::ResourceLimits { memory, processes })
    }

    /// Parse a ports block: `{ 8080 = 80  3000 = 3000 }`
    /// Maps host_port = container_port.
    fn parse_ports_block(&mut self) -> Result<Vec<crate::ast::PortMapping>, ParseError> {
        self.expect(&Token::LBrace)?;
        let mut mappings = Vec::new();

        loop {
            match self.peek() {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::IntLit(_) => {
                    let (tok, span) = self.advance();
                    let host = match tok {
                        Token::IntLit(n) => u16::try_from(n).map_err(|_| ParseError::InvalidInt {
                            value: n.to_string(),
                            span: span.clone(),
                        })?,
                        _ => unreachable!(),
                    };
                    self.expect(&Token::Equals)?;
                    let (tok, span) = self.advance();
                    let container = match tok {
                        Token::IntLit(n) => u16::try_from(n).map_err(|_| ParseError::InvalidInt {
                            value: n.to_string(),
                            span,
                        })?,
                        _ => {
                            return Err(ParseError::UnexpectedToken {
                                found: tok.to_string(),
                                span,
                                expected: "port number".into(),
                            });
                        }
                    };
                    mappings.push(crate::ast::PortMapping { host, container });
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "'}'".into(),
                    });
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "port number or '}'".into(),
                    });
                }
            }
        }

        Ok(mappings)
    }

    /// Parse a volumes block: `{ "mydata" = "/app/data" }`
    /// Maps volume_name = container_path.
    fn parse_volumes_block(&mut self) -> Result<Vec<crate::ast::VolumeMount>, ParseError> {
        self.expect(&Token::LBrace)?;
        let mut mounts = Vec::new();

        loop {
            match self.peek() {
                Token::RBrace => {
                    self.advance();
                    break;
                }
                Token::StringLit(_) | Token::Ident(_) => {
                    let (tok, _span) = self.advance();
                    let name = match tok {
                        Token::StringLit(s) | Token::Ident(s) => s,
                        _ => unreachable!(),
                    };
                    self.expect(&Token::Equals)?;
                    let container_path = self.parse_string()?;
                    mounts.push(crate::ast::VolumeMount { name, container_path });
                }
                Token::Eof => {
                    return Err(ParseError::UnexpectedEof {
                        expected: "'}'".into(),
                    });
                }
                _ => {
                    let (tok, span) = self.advance();
                    return Err(ParseError::UnexpectedToken {
                        found: tok.to_string(),
                        span,
                        expected: "volume name or '}'".into(),
                    });
                }
            }
        }

        Ok(mounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_cellfile() {
        let spec = Parser::parse(r#"cell { name = "test" }"#).unwrap();
        assert_eq!(spec.name, "test");
        assert!(spec.base.is_none());
        assert!(spec.env.is_empty());
        assert!(spec.fs_ops.is_empty());
        assert!(spec.run.is_none());
        assert!(spec.expose.is_empty());
    }

    #[test]
    fn test_full_cellfile() {
        let input = r#"
            cell {
                name = "myapp"
                base = "alpine:3.19"
                env {
                    NODE_ENV = "production"
                    PORT = "3000"
                }
                fs {
                    copy "src/" to "/app/src"
                    copy "package.json" to "/app/"
                }
                run = "/app/start.sh"
                expose = [8080, 3000]
            }
        "#;
        let spec = Parser::parse(input).unwrap();
        assert_eq!(spec.name, "myapp");
        assert_eq!(spec.base.as_deref(), Some("alpine:3.19"));
        assert_eq!(spec.env.len(), 2);
        assert_eq!(spec.env[0].key, "NODE_ENV");
        assert_eq!(spec.env[0].value, "production");
        assert_eq!(spec.env[1].key, "PORT");
        assert_eq!(spec.env[1].value, "3000");
        assert_eq!(spec.fs_ops.len(), 2);
        assert_eq!(
            spec.fs_ops[0],
            FsOp::Copy {
                src: "src/".into(),
                dest: "/app/src".into()
            }
        );
        assert_eq!(spec.run.as_deref(), Some("/app/start.sh"));
        assert_eq!(spec.expose, vec![8080, 3000]);
    }

    #[test]
    fn test_missing_name() {
        let result = Parser::parse(r#"cell { base = "alpine" }"#);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ParseError::MissingField { ref field } if field == "name"));
    }

    #[test]
    fn test_duplicate_field() {
        let result = Parser::parse(r#"cell { name = "a" name = "b" }"#);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ParseError::DuplicateField { .. }
        ));
    }

    #[test]
    fn test_unknown_field() {
        let result = Parser::parse(r#"cell { name = "a" bogus = "b" }"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_comments_in_cellfile() {
        let input = r#"
            # This is a Cellfile
            cell {
                # The app name
                name = "myapp"
                # Expose port
                expose = [80]
            }
        "#;
        let spec = Parser::parse(input).unwrap();
        assert_eq!(spec.name, "myapp");
        assert_eq!(spec.expose, vec![80]);
    }

    #[test]
    fn test_empty_blocks() {
        let input = r#"
            cell {
                name = "empty"
                env {}
                fs {}
                expose = []
            }
        "#;
        let spec = Parser::parse(input).unwrap();
        assert_eq!(spec.name, "empty");
        assert!(spec.env.is_empty());
        assert!(spec.fs_ops.is_empty());
        assert!(spec.expose.is_empty());
    }

    #[test]
    fn test_trailing_comma_in_array() {
        let input = r#"cell { name = "t" expose = [80, 443,] }"#;
        let spec = Parser::parse(input).unwrap();
        assert_eq!(spec.expose, vec![80, 443]);
    }
}
