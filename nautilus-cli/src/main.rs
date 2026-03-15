use anyhow::Result;
use clap::{Parser, Subcommand};

mod aws;
mod build;
mod init_ci;
mod attest;

/// Nautilus-Ops CLI — Self-Managed TEE Orchestrator for AWS Nitro Enclaves on Sui
#[derive(Parser)]
#[command(
    name = "nautilus",
    version,
    about = "Build, deploy and attest AWS Nitro Enclaves on the Sui blockchain",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build a Nitro Enclave Image (.eif) from a Dockerfile and extract PCR measurements
    Build(build::BuildArgs),

    /// Scaffold a GitHub Actions CI/CD workflow for automated enclave deployment to EC2
    InitCi(init_ci::InitCiArgs),

    /// Retrieve and verify the attestation document from a running enclave
    Attest(attest::AttestArgs),

    /// Verify that an EC2 instance has Nitro Enclave support enabled
    Verify(VerifyArgs),
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
        Commands::Build(args) => build::run(args).await,
        Commands::InitCi(args) => init_ci::run(args).await,
        Commands::Attest(args) => attest::run(args).await,
        Commands::Verify(args) => aws::verify_enclave_enabled(&args.instance_id).await,
    }
}
