use std::fs;

use anyhow::{Context, Result};

pub fn convert(path: &str) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read {path}"))?;

    let cellfile = dockerfile_to_cellfile(&content);

    println!("{}", cellfile);
    Ok(())
}

/// Parse a Dockerfile and produce an approximate Cellfile representation.
///
/// This handles the most common directives: FROM, ENV, COPY, RUN, EXPOSE,
/// WORKDIR, ENTRYPOINT, and CMD.
fn dockerfile_to_cellfile(dockerfile: &str) -> String {
    let mut base = String::from("scratch");
    let mut env_vars: Vec<(String, String)> = Vec::new();
    let mut copies: Vec<(String, String)> = Vec::new();
    let mut run_cmds: Vec<String> = Vec::new();
    let mut expose_ports: Vec<u16> = Vec::new();
    let mut entrypoint: Option<String> = None;
    let mut cmd: Option<String> = None;
    let mut workdir: Option<String> = None;

    // Join continuation lines (trailing '\').
    let joined = join_continuation_lines(dockerfile);

    for raw_line in joined.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Split into directive and arguments.
        let (directive, args) = match line.split_once(char::is_whitespace) {
            Some((d, a)) => (d.to_uppercase(), a.trim().to_string()),
            None => continue,
        };

        match directive.as_str() {
            "FROM" => {
                // Ignore --platform=... and AS alias.
                let img = args
                    .split_whitespace()
                    .find(|s| !s.starts_with("--"))
                    .unwrap_or("scratch");
                base = img.to_string();
            }
            "ENV" => {
                // ENV supports both `ENV KEY=VALUE` and `ENV KEY VALUE` forms.
                if let Some((key, value)) = parse_env_arg(&args) {
                    env_vars.push((key, value));
                }
            }
            "COPY" | "ADD" => {
                // Skip --from=... and --chown=... flags.
                let tokens: Vec<&str> = args
                    .split_whitespace()
                    .filter(|t| !t.starts_with("--"))
                    .collect();
                if tokens.len() >= 2 {
                    let dest = tokens.last().unwrap().to_string();
                    for src in &tokens[..tokens.len() - 1] {
                        copies.push((src.to_string(), dest.clone()));
                    }
                }
            }
            "RUN" => {
                run_cmds.push(args.clone());
            }
            "EXPOSE" => {
                for token in args.split_whitespace() {
                    // Accept "80/tcp" or just "80".
                    let port_str = token.split('/').next().unwrap_or(token);
                    if let Ok(p) = port_str.parse::<u16>() {
                        expose_ports.push(p);
                    }
                }
            }
            "WORKDIR" => {
                workdir = Some(args.clone());
            }
            "ENTRYPOINT" => {
                entrypoint = Some(parse_exec_form(&args));
            }
            "CMD" => {
                cmd = Some(parse_exec_form(&args));
            }
            _ => {
                // Ignore unknown directives (LABEL, ARG, VOLUME, USER, etc.).
            }
        }
    }

    // --- emit Cellfile ---
    let mut out = String::new();

    out.push_str("cell {\n");
    out.push_str(&format!("    name = \"{}\"\n", image_short_name(&base)));
    out.push_str(&format!("    base = \"{}\"\n", base));

    if !env_vars.is_empty() {
        out.push_str("    env {\n");
        for (k, v) in &env_vars {
            out.push_str(&format!("        {} = \"{}\"\n", k, escape_string(v)));
        }
        out.push_str("    }\n");
    }

    if !copies.is_empty() {
        out.push_str("    fs {\n");
        for (src, dest) in &copies {
            out.push_str(&format!(
                "        copy \"{}\" to \"{}\"\n",
                escape_string(src),
                escape_string(dest),
            ));
        }
        out.push_str("    }\n");
    }

    // Combine entrypoint, cmd, and workdir into a `run` field.
    let run_command = build_run_field(entrypoint.as_deref(), cmd.as_deref(), workdir.as_deref());
    if let Some(run) = &run_command {
        out.push_str(&format!("    run = \"{}\"\n", escape_string(run)));
    } else if !run_cmds.is_empty() {
        // Use the last RUN as the default command (heuristic).
        let last = run_cmds.last().unwrap();
        out.push_str(&format!("    run = \"{}\"\n", escape_string(last)));
    }

    if !expose_ports.is_empty() {
        let ports: Vec<String> = expose_ports.iter().map(|p| p.to_string()).collect();
        out.push_str(&format!("    expose = [{}]\n", ports.join(", ")));
    }

    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn join_continuation_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        if line.ends_with('\\') {
            result.push_str(&line[..line.len() - 1]);
            result.push(' ');
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

/// Parse `ENV` arguments — supports `KEY=VALUE` and legacy `KEY VALUE`.
fn parse_env_arg(args: &str) -> Option<(String, String)> {
    if let Some(eq_pos) = args.find('=') {
        let key = args[..eq_pos].trim().to_string();
        let value = args[eq_pos + 1..].trim().trim_matches('"').to_string();
        Some((key, value))
    } else {
        // Legacy form: ENV KEY VALUE
        let mut parts = args.splitn(2, char::is_whitespace);
        let key = parts.next()?.trim().to_string();
        let value = parts.next().unwrap_or("").trim().trim_matches('"').to_string();
        Some((key, value))
    }
}

/// Parse a JSON exec form (`["cmd", "arg"]`) or leave as shell form.
fn parse_exec_form(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.starts_with('[') {
        // Try JSON parse.
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
            return arr.join(" ");
        }
    }
    trimmed.to_string()
}

/// Combine ENTRYPOINT and CMD into a run field.
fn build_run_field(
    entrypoint: Option<&str>,
    cmd: Option<&str>,
    workdir: Option<&str>,
) -> Option<String> {
    let base = match (entrypoint, cmd) {
        (Some(ep), Some(c)) => Some(format!("{} {}", ep, c)),
        (Some(ep), None) => Some(ep.to_string()),
        (None, Some(c)) => Some(c.to_string()),
        (None, None) => None,
    };

    match (base, workdir) {
        (Some(cmd), Some(dir)) => Some(format!("cd {} && {}", dir, cmd)),
        (Some(cmd), None) => Some(cmd),
        (None, Some(dir)) => Some(format!("cd {}", dir)),
        (None, None) => None,
    }
}

fn image_short_name(base: &str) -> String {
    // "nginx:1.25" -> "nginx", "gcr.io/foo/bar:v1" -> "bar"
    let name = base.split(':').next().unwrap_or(base);
    name.rsplit('/').next().unwrap_or(name).to_string()
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
