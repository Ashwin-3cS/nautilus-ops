// nautilus-cli/src/aws.rs
//
// AWS provider integration for verifying EC2 enclave support.
// Gated behind the `aws` feature because aws-sdk-ec2 is very large.

use anyhow::Result;

#[cfg(feature = "aws")]
use anyhow::Context;
#[cfg(feature = "aws")]
use aws_config::BehaviorVersion;
#[cfg(feature = "aws")]
use aws_sdk_ec2::Client;
#[cfg(feature = "aws")]
use colored::Colorize;

/// Checks that the given EC2 instance has enclave mode enabled.
#[cfg(feature = "aws")]
pub async fn verify_enclave_enabled(instance_id: &str) -> Result<()> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = Client::new(&config);

    let response = client
        .describe_instances()
        .instance_ids(instance_id)
        .send()
        .await
        .context("Failed to describe EC2 instance. Check your AWS credentials and instance ID.")?;

    let reservation = response
        .reservations()
        .first()
        .context("No reservations found — is the instance ID correct?")?;

    let instance = reservation
        .instances()
        .first()
        .context("No instances found in reservation.")?;

    let enclave_opts = instance
        .enclave_options()
        .context("Enclave options not present in instance metadata.")?;

    if enclave_opts.enabled().unwrap_or(false) {
        println!(
            "{} Instance {} has enclave mode {}",
            "✔".green().bold(),
            instance_id.cyan(),
            "ENABLED".green().bold()
        );
        Ok(())
    } else {
        anyhow::bail!(
            "Instance {} does NOT have enclave mode enabled.\n\
             Nitro Enclaves must be enabled at instance launch time via:\n  \
             aws ec2 run-instances --enclave-options Enabled=true ...",
            instance_id
        );
    }
}

/// Stub when the `aws` feature is not enabled.
#[cfg(not(feature = "aws"))]
pub async fn verify_enclave_enabled(_instance_id: &str) -> Result<()> {
    anyhow::bail!(
        "AWS support is not compiled in. Rebuild with:\n  \
         cargo build --features aws"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_aws_module_compiles() {}
}
