use cell_format::{CellSpec, EnvVar};

/// Convert OCI container config fields into a [`CellSpec`].
///
/// This is the bridge between a pulled Docker/OCI image and Cell's native
/// manifest format.
pub fn oci_config_to_cellspec(
    name: &str,
    env: &[String],
    entrypoint: &[String],
    cmd: &[String],
    exposed_ports: &[u16],
    workdir: Option<&str>,
) -> CellSpec {
    // --- env ---
    let env_vars: Vec<EnvVar> = env
        .iter()
        .filter_map(|e| {
            let (key, value) = e.split_once('=')?;
            Some(EnvVar {
                key: key.to_string(),
                value: value.to_string(),
            })
        })
        .collect();

    // --- run command ---
    let run = build_run_command(entrypoint, cmd);

    // --- base ---
    let base = format!("oci:{name}");

    // --- workdir ---
    // If a working directory is set we prepend a `cd` to the run command so
    // the semantics are preserved even though CellSpec has no dedicated
    // workdir field.
    let run = match (run, workdir) {
        (Some(cmd), Some(dir)) if !dir.is_empty() => {
            Some(format!("cd {dir} && {cmd}"))
        }
        (run, _) => run,
    };

    CellSpec {
        name: name.to_string(),
        base,
        env: env_vars,
        fs_ops: Vec::new(),
        run,
        expose: exposed_ports.to_vec(),
        limits: None,
    }
}

/// Combine entrypoint and cmd into a single shell command string.
fn build_run_command(entrypoint: &[String], cmd: &[String]) -> Option<String> {
    let parts: Vec<&str> = entrypoint
        .iter()
        .chain(cmd.iter())
        .map(|s| s.as_str())
        .collect();

    if parts.is_empty() {
        return None;
    }

    Some(shell_join(&parts))
}

/// Join tokens into a shell-safe command string.
fn shell_join(tokens: &[&str]) -> String {
    tokens
        .iter()
        .map(|t| {
            if t.contains(' ') || t.contains('"') || t.contains('\'') || t.contains('\\') {
                format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                (*t).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Generate a Cellfile text representation from a [`CellSpec`].
pub fn cellspec_to_cellfile(spec: &CellSpec) -> String {
    let mut out = String::new();

    out.push_str(&format!("FROM \"{}\"\n", spec.base));
    out.push('\n');

    if !spec.env.is_empty() {
        out.push_str("env {\n");
        for var in &spec.env {
            out.push_str(&format!("    {} \"{}\"\n", var.key, var.value));
        }
        out.push_str("}\n\n");
    }

    for op in &spec.fs_ops {
        match op {
            cell_format::FsOp::Copy { src, dest } => {
                out.push_str(&format!("COPY \"{}\" \"{}\"\n", src, dest));
            }
        }
    }
    if !spec.fs_ops.is_empty() {
        out.push('\n');
    }

    if let Some(run) = &spec.run {
        out.push_str(&format!("RUN \"{}\"\n\n", run));
    }

    if !spec.expose.is_empty() {
        for port in &spec.expose {
            out.push_str(&format!("EXPOSE {port}\n"));
        }
        out.push('\n');
    }

    if let Some(limits) = &spec.limits {
        out.push_str("limits {\n");
        if let Some(mem) = limits.memory {
            out.push_str(&format!("    memory {mem}\n"));
        }
        if let Some(procs) = limits.processes {
            out.push_str(&format!("    processes {procs}\n"));
        }
        out.push_str("}\n\n");
    }

    // Trim trailing blank lines, keep one final newline.
    let trimmed = out.trim_end();
    let mut result = trimmed.to_string();
    result.push('\n');
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_conversion() {
        let spec = oci_config_to_cellspec(
            "nginx",
            &["PATH=/usr/bin".to_string()],
            &["nginx".to_string()],
            &["-g".to_string(), "daemon off;".to_string()],
            &[80, 443],
            Some("/app"),
        );

        assert_eq!(spec.name, "nginx");
        assert_eq!(spec.base, "oci:nginx");
        assert_eq!(spec.env.len(), 1);
        assert_eq!(spec.env[0].key, "PATH");
        assert_eq!(spec.env[0].value, "/usr/bin");
        assert!(spec.run.is_some());
        let run = spec.run.as_ref().unwrap();
        assert!(run.contains("nginx"));
        assert!(run.contains("daemon off;"));
        assert!(run.starts_with("cd /app && "));
        assert_eq!(spec.expose, vec![80, 443]);
    }

    #[test]
    fn empty_entrypoint_and_cmd() {
        let spec = oci_config_to_cellspec("scratch", &[], &[], &[], &[], None);
        assert!(spec.run.is_none());
        assert!(spec.env.is_empty());
    }

    #[test]
    fn cellfile_round_trip() {
        let spec = oci_config_to_cellspec(
            "myapp",
            &["FOO=bar".to_string()],
            &["/bin/sh".to_string()],
            &[],
            &[8080],
            None,
        );

        let text = cellspec_to_cellfile(&spec);
        assert!(text.contains("FROM \"oci:myapp\""));
        assert!(text.contains("FOO \"bar\""));
        assert!(text.contains("EXPOSE 8080"));
        assert!(text.contains("RUN"));
    }

    #[test]
    fn cellfile_empty_spec() {
        let spec = oci_config_to_cellspec("empty", &[], &[], &[], &[], None);
        let text = cellspec_to_cellfile(&spec);
        assert!(text.contains("FROM \"oci:empty\""));
        assert!(!text.contains("env {"));
        assert!(!text.contains("RUN"));
    }

    #[test]
    fn env_parsing_no_value() {
        let spec = oci_config_to_cellspec("test", &["NOVALUE".to_string()], &[], &[], &[], None);
        // Keys without '=' are silently dropped.
        assert!(spec.env.is_empty());
    }

    #[test]
    fn env_parsing_empty_value() {
        let spec = oci_config_to_cellspec("test", &["KEY=".to_string()], &[], &[], &[], None);
        assert_eq!(spec.env.len(), 1);
        assert_eq!(spec.env[0].key, "KEY");
        assert_eq!(spec.env[0].value, "");
    }
}
