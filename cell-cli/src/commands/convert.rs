use anyhow::{Context, Result};

pub fn execute(path: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read '{path}'"))?;

    let json_mode = super::is_json();

    if !json_mode {
        println!("# Auto-generated Cellfile from {path}");
        println!("# Review and customize before building.\n");
    }

    let mut name = "myapp".to_string();
    let mut base = None;
    let mut env_vars = Vec::new();
    let mut run_cmd = None;
    let mut expose = Vec::new();
    let mut copies = Vec::new();
    let mut volumes = Vec::new();
    let mut workdir: Option<String> = None;
    let mut labels = Vec::new();
    let mut _user: Option<String> = None;
    // Track multi-stage: only keep the last FROM's context
    let mut stage_count = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Handle line continuations
        let (instruction, args) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let args = args.trim();

        match instruction.to_uppercase().as_str() {
            "FROM" => {
                stage_count += 1;
                if stage_count > 1 {
                    // Multi-stage: reset everything for the final stage
                    env_vars.clear();
                    copies.clear();
                    expose.clear();
                    volumes.clear();
                    run_cmd = None;
                    workdir = None;
                }
                // Handle "FROM image AS name" — strip the AS clause
                let image = args.split_whitespace().next().unwrap_or(args);
                base = Some(image.to_string());
                if let Some(img_name) = image.split('/').last() {
                    name = img_name.split(':').next().unwrap_or(img_name).to_string();
                }
            }
            "ENV" => {
                // Handle both "ENV KEY=VALUE" and "ENV KEY VALUE" formats
                if args.contains('=') {
                    // Could be multiple KEY=VALUE pairs
                    for pair in args.split_whitespace() {
                        if pair.contains('=') {
                            env_vars.push(pair.to_string());
                        }
                    }
                } else if let Some((k, v)) = args.split_once(char::is_whitespace) {
                    env_vars.push(format!("{}={}", k, v));
                }
            }
            "ARG" => {
                // ARG with default value becomes an env var
                if let Some((k, v)) = args.split_once('=') {
                    env_vars.push(format!("{}={}", k, v));
                }
            }
            "COPY" | "ADD" => {
                // Handle --from=stage and --chown flags
                let mut parts: Vec<&str> = args.split_whitespace().collect();
                // Strip flags
                parts.retain(|p| !p.starts_with("--"));
                if parts.len() >= 2 {
                    let dest = parts.pop().unwrap().to_string();
                    for src in &parts {
                        copies.push((src.to_string(), dest.clone()));
                    }
                }
            }
            "ENTRYPOINT" => {
                run_cmd = Some(parse_exec_form(args));
            }
            "CMD" => {
                // CMD is used if no ENTRYPOINT exists, or appended to ENTRYPOINT
                if run_cmd.is_none() {
                    run_cmd = Some(parse_exec_form(args));
                }
            }
            "EXPOSE" => {
                for port_str in args.split_whitespace() {
                    let port_str = port_str.split('/').next().unwrap_or(port_str);
                    if let Ok(port) = port_str.parse::<u16>() {
                        expose.push(port);
                    }
                }
            }
            "WORKDIR" => {
                workdir = Some(args.to_string());
            }
            "VOLUME" => {
                // VOLUME ["/data"] or VOLUME /data /logs
                let clean = args
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .replace('"', "");
                for vol_path in clean.split(',') {
                    let vol_path = vol_path.trim();
                    if !vol_path.is_empty() {
                        // Generate a volume name from the path
                        let vol_name = vol_path
                            .trim_start_matches('/')
                            .replace('/', "-");
                        volumes.push(cell_format::VolumeMount {
                            name: vol_name,
                            container_path: vol_path.to_string(),
                        });
                    }
                }
            }
            "LABEL" => {
                labels.push(args.to_string());
            }
            "USER" => {
                _user = Some(args.to_string());
            }
            "HEALTHCHECK" | "STOPSIGNAL" | "SHELL" | "RUN" | "ONBUILD" => {
                // Skipped — not representable in Cellfile (yet)
            }
            _ => {}
        }
    }

    // Generate port mappings from expose (default: same port on host)
    let port_mappings: Vec<cell_format::PortMapping> = expose
        .iter()
        .map(|&p| cell_format::PortMapping {
            host: p,
            container: p,
        })
        .collect();

    let spec = cell_format::CellSpec {
        name,
        base,
        env: env_vars
            .iter()
            .filter_map(|e| {
                let (k, v) = e.split_once('=')?;
                Some(cell_format::EnvVar {
                    key: k.trim().to_string(),
                    value: v.trim().to_string(),
                })
            })
            .collect(),
        fs_ops: copies
            .iter()
            .map(|(src, dest)| cell_format::FsOp::Copy {
                src: src.clone(),
                dest: dest.clone(),
            })
            .collect(),
        run: run_cmd,
        expose,
        limits: None,
        ports: port_mappings,
        volumes,
    };

    let cellfile_text = cell_oci::convert::cellspec_to_cellfile(&spec);

    if json_mode {
        // Send informational notes to stderr so they don't pollute the JSON
        if stage_count > 1 {
            eprintln!("# Note: Multi-stage build detected ({stage_count} stages). Only the final stage was converted.");
        }
        if !labels.is_empty() {
            eprintln!("# Labels from Dockerfile:");
            for label in &labels {
                eprintln!("#   {label}");
            }
        }
        if let Some(ref wd) = workdir {
            eprintln!("# WORKDIR was: {wd}");
        }
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "cellfile": cellfile_text
            }))?
        );
    } else {
        // Print as comments any info we couldn't represent
        if stage_count > 1 {
            println!("# Note: Multi-stage build detected ({stage_count} stages). Only the final stage was converted.");
        }
        if !labels.is_empty() {
            println!("# Labels from Dockerfile:");
            for label in &labels {
                println!("#   {label}");
            }
            println!();
        }
        if let Some(ref wd) = workdir {
            println!("# WORKDIR was: {wd}");
            println!();
        }
        print!("{}", cellfile_text);
    }
    Ok(())
}

/// Parse Docker exec form ["cmd", "arg1"] or shell form "cmd arg1"
fn parse_exec_form(args: &str) -> String {
    if args.starts_with('[') {
        args.trim_start_matches('[')
            .trim_end_matches(']')
            .replace('"', "")
            .split(',')
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        args.to_string()
    }
}
