use agentmarket::commands;

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
    Search {
        /// Filter by capability
        #[arg(short, long)]
        capability: Option<String>,
        /// Search for open requests instead of agents
        #[arg(short, long)]
        requests: bool,
    },
    /// Create a service request for another agent
    Request {
        /// Task description
        #[arg(short, long)]
        task: String,
        /// Price in USD (e.g., 5.00)
        #[arg(short, long)]
        price: f64,
        /// Deadline in hours from now
        #[arg(short, long, default_value = "24")]
        deadline: u64,
        /// Target agent ID (optional, 0 for open request)
        #[arg(long, default_value = "0")]
        to: u64,
        /// Path to a file to attach (optional)
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Submit a response to a request
    Respond {
        /// Request ID to respond to
        #[arg(short = 'i', long)]
        request_id: String,
        /// Path to the deliverable file
        #[arg(short, long)]
        file: Option<String>,
        /// Response message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Enter the validation loop to review and earn
    Validate,
    /// Settle a validated response and trigger payment
    Claim {
        /// Request ID to claim payment for
        #[arg(short = 'i', long)]
        request_id: String,
    },
    /// View agent status, earnings, and reputation
    Status,
    /// Move earned USDC to an external address
    Withdraw,
    /// Run validate + auto-claim as a continuous loop
    Daemon,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter =
        EnvFilter::try_from_env("AGENTMARKET_LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("warn"));

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
        Commands::Search {
            capability,
            requests,
        } => commands::search::run(capability, requests).await,
        Commands::Request {
            task,
            price,
            deadline,
            to,
            file,
        } => commands::request::run(task, price, deadline, to, file).await,
        Commands::Respond {
            request_id,
            file,
            message,
        } => commands::respond::run(request_id, file, message).await,
        Commands::Validate => commands::validate::run().await,
        Commands::Claim { request_id } => commands::claim::run(request_id).await,
        Commands::Status => commands::status::run().await,
        Commands::Withdraw => commands::withdraw::run().await,
        Commands::Daemon => commands::daemon::run().await,
    }
}
