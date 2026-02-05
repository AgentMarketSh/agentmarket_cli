mod chain;
mod commands;
mod config;
mod engine;
mod ipfs;
mod output;

use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(name = "agentmarket")]
#[command(about = "Trust infrastructure for the autonomous agent economy")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate agent identity and local configuration
    Init,
    /// Display wallet address and ETH balance for funding
    Fund,
    /// Register agent on-chain via ERC-8004
    Register,
    /// Discover agents and open requests
    Search,
    /// Create a service request for another agent
    Request,
    /// Submit a response to a request
    Respond,
    /// Enter the validation loop to review and earn
    Validate,
    /// Settle a validated response and trigger payment
    Claim,
    /// View agent status, earnings, and reputation
    Status,
    /// Move earned USDC to an external address
    Withdraw,
    /// Run validate + auto-claim as a continuous loop
    Daemon,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("AGENTMARKET_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("warn"));

    fmt::Subscriber::builder()
        .with_env_filter(filter)
        .compact()
        .with_timer(fmt::time::SystemTime)
        .init();

    let cli = Cli::parse();

    tracing::debug!("command dispatched");

    match cli.command {
        Commands::Init => commands::init::run().await,
        Commands::Fund => commands::fund::run().await,
        Commands::Register => commands::register::run().await,
        Commands::Search => commands::search::run().await,
        Commands::Request => commands::request::run().await,
        Commands::Respond => commands::respond::run().await,
        Commands::Validate => commands::validate::run().await,
        Commands::Claim => commands::claim::run().await,
        Commands::Status => commands::status::run().await,
        Commands::Withdraw => commands::withdraw::run().await,
        Commands::Daemon => commands::daemon::run().await,
    }
}
