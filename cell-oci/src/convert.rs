use cell_format::{CellSpec, EnvVar, PortMapping, VolumeMount};

/// Convert an OCI image config into a CellSpec.
///
/// Maps standard OCI/Docker config fields to Cell equivalents:
/// - `Env` → `env {}`
/// - `Entrypoint` / `Cmd` → `run`
/// - `ExposedPorts` → `expose`
/// - `WorkingDir` → (future: workdir field)
pub fn oci_config_to_cellspec(
    name: &str,
    env: &[String],
    entrypoint: Option<&str>,
    exposed_ports: &[u16],
    ports: Vec<PortMapping>,
    volumes: Vec<VolumeMount>,
) -> CellSpec {
    let env_vars = env
        .iter()
        .filter_map(|e| {
            let (k, v) = e.split_once('=')?;
            Some(EnvVar {
                key: k.to_string(),
                value: v.to_string(),
            })
        })
        .collect();

    CellSpec {
        name: name.to_string(),
        base: None,
        env: env_vars,
        fs_ops: vec![],
        run: entrypoint.map(|s| s.to_string()),
        expose: exposed_ports.to_vec(),
        limits: None,
        ports,
        volumes,
    }
}

/// Generate a Cellfile source string from a CellSpec.
pub fn cellspec_to_cellfile(spec: &CellSpec) -> String {
    let mut out = String::new();
    out.push_str("cell {\n");
    out.push_str(&format!("  name = \"{}\"\n", spec.name));

    if let Some(ref base) = spec.base {
        out.push_str(&format!("  base = \"{base}\"\n"));
    }

    if !spec.env.is_empty() {
        out.push_str("  env {\n");
        for var in &spec.env {
            out.push_str(&format!("    {} = \"{}\"\n", var.key, var.value));
        }
        out.push_str("  }\n");
    }

    if !spec.fs_ops.is_empty() {
        out.push_str("  fs {\n");
        for op in &spec.fs_ops {
            match op {
                cell_format::FsOp::Copy { src, dest } => {
                    out.push_str(&format!("    copy \"{src}\" to \"{dest}\"\n"));
                }
            }
        }
        out.push_str("  }\n");
    }

    if let Some(ref run) = spec.run {
        out.push_str(&format!("  run = \"{run}\"\n"));
    }

    if !spec.expose.is_empty() {
        let expose: Vec<String> = spec.expose.iter().map(|p| p.to_string()).collect();
        out.push_str(&format!("  expose = [{}]\n", expose.join(", ")));
    }

    if !spec.ports.is_empty() {
        out.push_str("  ports {\n");
        for pm in &spec.ports {
            out.push_str(&format!("    {} = {}\n", pm.host, pm.container));
        }
        out.push_str("  }\n");
    }

    if !spec.volumes.is_empty() {
        out.push_str("  volumes {\n");
        for vm in &spec.volumes {
            out.push_str(&format!("    \"{}\" = \"{}\"\n", vm.name, vm.container_path));
        }
        out.push_str("  }\n");
    }

    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oci_to_cellspec() {
        let spec = oci_config_to_cellspec(
            "nginx",
            &["PATH=/usr/bin".into(), "NGINX_VERSION=1.25".into()],
            Some("nginx -g 'daemon off;'"),
            &[80, 443],
            vec![],
            vec![],
        );
        assert_eq!(spec.name, "nginx");
        assert_eq!(spec.env.len(), 2);
        assert_eq!(spec.env[0].key, "PATH");
        assert_eq!(spec.run.as_deref(), Some("nginx -g 'daemon off;'"));
        assert_eq!(spec.expose, vec![80, 443]);
    }

    #[test]
    fn test_cellspec_to_cellfile_roundtrip() {
        let spec = oci_config_to_cellspec("myapp", &["KEY=val".into()], Some("/start.sh"), &[8080], vec![], vec![]);
        let cellfile = cellspec_to_cellfile(&spec);
        // Parse it back
        let parsed = cell_format::Parser::parse(&cellfile).unwrap();
        assert_eq!(parsed.name, "myapp");
        assert_eq!(parsed.env[0].key, "KEY");
        assert_eq!(parsed.run.as_deref(), Some("/start.sh"));
        assert_eq!(parsed.expose, vec![8080]);
    }
}
