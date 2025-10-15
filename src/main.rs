
mod config;
mod fim;
mod metrics;

use clap::{Parser, Subcommand};
use tracing::{Level};
use tracing_subscriber::EnvFilter;
use anyhow::Result;

#[derive(Parser, Debug)]
#[command(name = "sentra_fim", about = "File Integrity Monitor with Prometheus & JSONL")]
struct Cli {
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build baseline (SQLite) for configured paths
    Init {
        #[arg(short, long, default_value = "config.toml")]
        config: String,
    },
    /// Watch filesystem, update baseline, emit JSONL and metrics
    Watch {
        #[arg(short, long, default_value = "config.toml")]
        config: String,
        /// JSONL audit file (append)
        #[arg(short, long, default_value = "events.jsonl")]
        jsonl: String,
    },
    /// Offline compare current state vs baseline
    Scan {
        #[arg(short, long, default_value = "config.toml")]
        config: String,
        /// Optional JSONL diff output
        #[arg(long)]
        jsonl: Option<String>,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let level = if cli.verbose { Level::DEBUG } else { Level::INFO };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env()
            .add_directive(level.into()))
        .with_target(false)
        .compact()
        .init();

    match cli.command {
        Commands::Init { config } => {
            let cfg = config::Config::load(&config)?;
            fim::build_baseline(&cfg)?;
            println!("Baseline built at {}", cfg.baseline_db);
        }
        Commands::Watch { config, jsonl } => {
            let cfg = config::Config::load(&config)?;
            let prom = metrics::Metrics::try_new()?;
            let http = metrics::serve_metrics(cfg.metrics_bind.clone(), prom.registry()).await?;
            let _g = http; // keep server alive

            fim::watch_loop(cfg, jsonl, prom).await?;
        }
        Commands::Scan { config, jsonl } => {
            let cfg = config::Config::load(&config)?;
            fim::scan_diff(&cfg, jsonl)?;
        }
    }
    Ok(())
}
