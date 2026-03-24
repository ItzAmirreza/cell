mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cell", about = "Cell container runtime", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build an image from a Cellfile.
    Build {
        /// Path to the Cellfile (or directory containing one).
        path: String,
    },
    /// Run a container from an image.
    Run {
        /// Image name to run.
        image: String,
        /// Command and arguments to execute inside the container (overrides default).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// List locally stored images.
    Images,
    /// List containers.
    Ps,
    /// Remove a container by id (or prefix).
    Rm {
        /// Container id or prefix.
        id: String,
    },
    /// Pull an image from a remote registry.
    Pull {
        /// Image reference (e.g. "nginx", "ghcr.io/owner/repo:v1").
        reference: String,
    },
    /// Convert a Dockerfile to a Cellfile.
    Convert {
        /// Path to the Dockerfile.
        path: String,
    },
    /// Run a command inside an existing container.
    Exec {
        /// Container id or prefix.
        id: String,
        /// Command and arguments to execute inside the container.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Stop a running container (SIGTERM, then SIGKILL after 5s).
    Stop {
        /// Container id or prefix.
        id: String,
    },
    /// Display runtime isolation information.
    Info,
}

/// Join command-line argument tokens back into a single command string.
///
/// When exactly one argument is provided it is passed through verbatim — the
/// user already composed the full command (e.g. `"/bin/sh -c 'echo hi'"`).
/// When multiple arguments are given (e.g. `/bin/sh -c "echo hi"`) they are
/// joined with proper quoting so the downstream `shell_split` can recover
/// the original tokens.
fn shell_join_args(args: &[String]) -> String {
    if args.len() == 1 {
        return args[0].clone();
    }
    args.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('\'') || a.contains('"') || a.contains('\\') {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Build { path } => commands::build::build(&path),
        Command::Run { image, command } => {
            let cmd = if command.is_empty() {
                None
            } else {
                Some(shell_join_args(&command))
            };
            commands::run::run(&image, cmd.as_deref())
        }
        Command::Images => commands::images::images(),
        Command::Ps => commands::ps::ps(),
        Command::Rm { id } => commands::rm::rm(&id),
        Command::Pull { reference } => commands::pull::pull(&reference),
        Command::Convert { path } => commands::convert::convert(&path),
        Command::Exec { id, command } => {
            let cmd = if command.is_empty() {
                "/bin/sh".to_string()
            } else {
                shell_join_args(&command)
            };
            commands::exec::exec(&id, &cmd)
        }
        Command::Stop { id } => commands::stop::stop(&id),
        Command::Info => commands::info::info(),
    }
}
