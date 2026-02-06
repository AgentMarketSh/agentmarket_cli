use agentmarket::commands;
use agentmarket::output::formatter;

use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(name = "agentmarket")]
#[command(about = "Trust infrastructure for the autonomous agent economy")]
#[command(version)]
struct Cli {
    /// Output in JSON format for machine consumption
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate agent identity and local configuration
    Init {
        /// Agent name (skip interactive prompt)
        #[arg(long)]
        name: Option<String>,
        /// Agent description (skip interactive prompt)
        #[arg(long)]
        description: Option<String>,
        /// Capabilities, comma-separated (skip interactive prompt)
        #[arg(long)]
        capabilities: Option<String>,
        /// Price per task in USD (skip interactive prompt)
        #[arg(long)]
        price: Option<f64>,
    },
    /// Check agent balance and add funds
    Fund,
    /// Register agent on the network
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
    Validate {
        /// Handler type: "manual" or "external"
        #[arg(long, default_value = "manual")]
        handler: String,
        /// Path to external handler executable
        #[arg(long)]
        handler_path: Option<String>,
        /// Run continuously (poll for pending validations)
        #[arg(long)]
        auto: bool,
        /// Filter by capability
        #[arg(long)]
        filter: Option<String>,
    },
    /// Claim payment for completed work
    Claim {
        /// Request ID to claim payment for
        #[arg(short = 'i', long)]
        request_id: String,
    },
    /// View agent status, earnings, and reputation
    Status,
    /// Transfer earnings to another address
    Withdraw {
        /// Destination address (0x-prefixed)
        #[arg(short = 'a', long)]
        address: String,
        /// Amount in USD to withdraw (withdraws all if not specified)
        #[arg(long)]
        amount: Option<f64>,
    },
    /// Run validate + auto-claim as a continuous loop
    Daemon {
        /// Poll interval in seconds
        #[arg(long, default_value = "60")]
        interval: u64,
        /// Handler type for validation
        #[arg(long, default_value = "manual")]
        handler: String,
        /// Path to external handler executable
        #[arg(long)]
        handler_path: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let filter =
        EnvFilter::try_from_env("AGENTMARKET_LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("warn"));

    fmt::Subscriber::builder()
        .with_env_filter(filter)
        .compact()
        .with_timer(fmt::time::SystemTime)
        .init();

    let cli = Cli::parse();

    formatter::set_json_mode(cli.json);

    tracing::debug!("command dispatched");

    if let Err(err) = run_command(cli.command).await {
        formatter::print_error(&err);
        std::process::exit(1);
    }
}

async fn run_command(command: Commands) -> anyhow::Result<()> {
    match command {
        Commands::Init {
            name,
            description,
            capabilities,
            price,
        } => commands::init::run(name, description, capabilities, price).await,
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
        Commands::Validate {
            handler,
            handler_path,
            auto,
            filter,
        } => commands::validate::run(handler, handler_path, auto, filter).await,
        Commands::Claim { request_id } => commands::claim::run(request_id).await,
        Commands::Status => commands::status::run().await,
        Commands::Withdraw { address, amount } => commands::withdraw::run(address, amount).await,
        Commands::Daemon {
            interval,
            handler,
            handler_path,
        } => commands::daemon::run(interval, handler, handler_path).await,
    }
}
