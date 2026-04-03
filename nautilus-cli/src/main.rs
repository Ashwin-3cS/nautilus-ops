use anyhow::Result;
use clap::{Parser, Subcommand};

mod aws;
mod build;
pub mod config;
mod init;
mod init_ci;
mod attest;
mod status;
mod logs;
mod sui_chain;

/// Nautilus-Ops CLI — Self-Managed TEE Orchestrator for AWS Nitro Enclaves on Sui
#[derive(Parser)]
#[command(
    name = "nautilus",
    version,
    about = "Build, deploy and attest AWS Nitro Enclaves on the Sui blockchain",
    long_about = None,
)]
struct Cli {
    /// Template type (auto-detected from project if omitted).
    #[arg(long, global = true, value_enum)]
    template: Option<config::Template>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new Nautilus TEE project from a template (including messaging-relayer)
    Init(init::InitArgs),

    /// Build a Nitro Enclave Image (.eif) from a Dockerfile and extract PCR measurements
    Build(build::BuildArgs),

    /// Scaffold a GitHub Actions CI/CD workflow for automated enclave deployment to EC2
    InitCi(init_ci::InitCiArgs),

    /// Retrieve and verify the attestation document from a running enclave
    Attest(attest::AttestArgs),

    /// Check enclave health, attestation, and on-chain status
    Status(status::StatusArgs),

    /// Fetch recent logs from a running enclave
    Logs(logs::LogsArgs),

    /// Verify that an EC2 instance has Nitro Enclave support enabled
    Verify(VerifyArgs),

    /// Deploy the Nautilus Move contract to Sui
    DeployContract(sui_chain::DeployContractArgs),

    /// Register a running enclave on-chain with its attestation document
    RegisterEnclave(sui_chain::RegisterEnclaveArgs),

    /// Update expected PCR values in the on-chain EnclaveConfig
    UpdatePcrs(sui_chain::UpdatePcrsArgs),

    /// Verify an enclave signature on-chain using the selected template's signing flow
    VerifySignature(sui_chain::VerifySignatureArgs),
}

#[derive(clap::Args, Debug)]
struct VerifyArgs {
    /// EC2 instance ID to check (e.g. i-0abc123def456789a)
    #[arg(long, env = "TEE_EC2_INSTANCE_ID")]
    instance_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init(args) => init::run(args).await,
        Commands::Build(args) => build::run(args, cli.template).await,
        Commands::InitCi(args) => init_ci::run(args, cli.template).await,
        Commands::Attest(args) => attest::run(args, cli.template).await,
        Commands::Status(args) => status::run(args, cli.template).await,
        Commands::Logs(args) => logs::run(args, cli.template).await,
        Commands::Verify(args) => aws::verify_enclave_enabled(&args.instance_id).await,
        Commands::DeployContract(args) => sui_chain::deploy_contract(args).await,
        Commands::RegisterEnclave(args) => sui_chain::register_enclave(args, cli.template).await,
        Commands::UpdatePcrs(args) => sui_chain::update_pcrs(args).await,
        Commands::VerifySignature(args) => sui_chain::verify_signature(args, cli.template).await,
    }
}
