mod commands;
mod updater;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cell", version, about = "A modern container runtime")]
struct Cli {
    /// Enable verbose output (show per-event guard messages)
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Output JSON instead of human-readable text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a container from an image
    Run {
        /// Image name
        image: String,
        /// Command to execute (overrides entrypoint)
        command: Option<String>,
        /// Interactive mode (pipe stdin to the container)
        #[arg(short, long)]
        interactive: bool,
    },
    /// Build an image from a Cellfile
    Build {
        /// Path to Cellfile (default: ./Cellfile)
        #[arg(default_value = "Cellfile")]
        path: String,
    },
    /// List local images
    Images,
    /// List running containers
    Ps,
    /// Remove a container
    Rm {
        /// Container ID (or prefix)
        id: String,
    },
    /// Pull an OCI/Docker image and convert to Cell format
    Pull {
        /// Image reference (e.g., nginx:latest)
        reference: String,
    },
    /// Convert a Dockerfile to a Cellfile
    Convert {
        /// Path to Dockerfile
        path: String,
    },
    /// Run a command in an existing container
    Exec {
        /// Container ID (or prefix)
        id: String,
        /// Command to execute
        command: String,
        /// Interactive mode
        #[arg(short, long)]
        interactive: bool,
    },
    /// Show platform isolation capabilities
    Info,
    /// Stop a running container
    Stop {
        /// Container ID (or prefix)
        id: String,
    },
    /// Check for updates and upgrade to the latest version
    Upgrade,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    cell_runtime::VERBOSE.store(cli.verbose, std::sync::atomic::Ordering::Relaxed);
    commands::set_json_mode(cli.json);

    // Auto-update check on startup (non-blocking, cached 24h).
    // Skip for upgrade command (it does its own check) and JSON mode (don't pollute output).
    if !cli.json && !matches!(cli.command, Commands::Upgrade) {
        updater::startup_check();
    }

    match cli.command {
        Commands::Run { image, command, interactive } => commands::run::execute(&image, command.as_deref(), interactive),
        Commands::Build { path } => commands::build::execute(&path),
        Commands::Images => commands::images::execute(),
        Commands::Ps => commands::ps::execute(),
        Commands::Rm { id } => commands::rm::execute(&id),
        Commands::Pull { reference } => commands::pull::execute(&reference),
        Commands::Convert { path } => commands::convert::execute(&path),
        Commands::Exec { id, command, interactive } => commands::exec::execute(&id, &command, interactive),
        Commands::Info => commands::info::execute(),
        Commands::Stop { id } => commands::stop::execute(&id),
        Commands::Upgrade => updater::upgrade(),
    }
}
