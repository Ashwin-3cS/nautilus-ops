use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

// ─── Deploy Contract ────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct DeployContractArgs {
    /// Path to the Move package directory.
    #[arg(long, default_value = "contracts/nautilus")]
    pub move_path: PathBuf,

    /// Sui network to deploy to (devnet, testnet, mainnet).
    #[arg(long, default_value = "testnet")]
    pub network: String,

    /// Gas budget for the publish transaction.
    #[arg(long, default_value = "500000000")]
    pub gas_budget: u64,
}

// ─── Register Enclave ───────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct RegisterEnclaveArgs {
    /// EC2 host running the enclave (IP or DNS).
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// HTTP port of the sign-server (socat bridge to enclave).
    #[arg(long, default_value = "4000")]
    pub port: u16,

    /// Sui network (devnet, testnet, mainnet).
    #[arg(long, default_value = "testnet")]
    pub network: String,

    /// On-chain package ID of the deployed Nautilus contract.
    #[arg(long, env = "NAUTILUS_PACKAGE_ID")]
    pub package_id: Option<String>,

    /// On-chain EnclaveConfig object ID.
    #[arg(long, env = "NAUTILUS_CONFIG_ID")]
    pub config_object_id: Option<String>,

    /// On-chain Cap object ID.
    #[arg(long, env = "NAUTILUS_CAP_ID")]
    pub cap_object_id: Option<String>,

    /// Path to PCR file — if provided, updates on-chain PCRs before registering.
    #[arg(long)]
    pub pcr_file: Option<PathBuf>,

    /// Gas budget for the transaction.
    #[arg(long, default_value = "100000000")]
    pub gas_budget: u64,
}

// ─── Update PCRs ────────────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct UpdatePcrsArgs {
    /// Path to PCR JSON file (output of `nautilus build`).
    #[arg(long)]
    pub pcr_file: Option<PathBuf>,

    /// PCR0 hex value (alternative to --pcr-file).
    #[arg(long)]
    pub pcr0: Option<String>,

    /// PCR1 hex value.
    #[arg(long)]
    pub pcr1: Option<String>,

    /// PCR2 hex value.
    #[arg(long)]
    pub pcr2: Option<String>,

    /// Sui network (devnet, testnet, mainnet).
    #[arg(long, default_value = "testnet")]
    pub network: String,

    /// On-chain package ID.
    #[arg(long, env = "NAUTILUS_PACKAGE_ID")]
    pub package_id: Option<String>,

    /// On-chain EnclaveConfig object ID.
    #[arg(long, env = "NAUTILUS_CONFIG_ID")]
    pub config_object_id: Option<String>,

    /// On-chain Cap object ID.
    #[arg(long, env = "NAUTILUS_CAP_ID")]
    pub cap_object_id: Option<String>,

    /// Gas budget for the transaction.
    #[arg(long, default_value = "50000000")]
    pub gas_budget: u64,
}

// ─── Verify Signature ──────────────────────────────────────────────────

#[derive(Args, Debug)]
pub struct VerifySignatureArgs {
    /// EC2 host running the enclave (IP or DNS).
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// HTTP port of the sign-server.
    #[arg(long, default_value = "4000")]
    pub port: u16,

    /// Name to sign (sent to the enclave's /sign_name endpoint).
    #[arg(long, default_value = "Nautilus")]
    pub name: String,

    /// On-chain Enclave object ID (created by register-enclave).
    #[arg(long, env = "NAUTILUS_ENCLAVE_ID")]
    pub enclave_id: String,

    /// Sui network (devnet, testnet, mainnet).
    #[arg(long, default_value = "testnet")]
    pub network: String,

    /// On-chain package ID of the deployed Nautilus contract.
    #[arg(long, env = "NAUTILUS_PACKAGE_ID")]
    pub package_id: Option<String>,

    /// Gas budget for the transaction.
    #[arg(long, default_value = "50000000")]
    pub gas_budget: u64,
}

// ─── Feature-gated implementations ──────────────────────────────────────

#[cfg(feature = "sui")]
mod implementation {
    use super::*;
    use anyhow::Context;
    use colored::Colorize;
    use crate::config::NautilusConfig;

    /// Resolve an object ID from CLI arg, env var, or .nautilus.toml.
    fn resolve_id(
        cli_val: &Option<String>,
        config_val: &Option<String>,
        name: &str,
    ) -> Result<String> {
        cli_val
            .as_ref()
            .or(config_val.as_ref())
            .cloned()
            .with_context(|| format!(
                "Missing {}. Pass via --{} or set in .nautilus.toml",
                name,
                name.replace('_', "-")
            ))
    }

    pub async fn deploy_contract(args: DeployContractArgs) -> Result<()> {
        println!("{}", "Nautilus Contract Deployment".bold().cyan());
        println!("{}", "─".repeat(40).dimmed());

        let move_path = args.move_path.canonicalize()
            .with_context(|| format!(
                "Move package not found at {}.\n\
                 Run this command from the nautilus-ops root directory.",
                args.move_path.display()
            ))?;

        println!(
            "{} Publishing Move package from: {}",
            "→".cyan(),
            move_path.display()
        );
        println!(
            "{} Network: {}",
            "→".cyan(),
            args.network.cyan()
        );

        let output = std::process::Command::new("sui")
            .args([
                "client", "publish",
                "--json",
                "--gas-budget", &args.gas_budget.to_string(),
            ])
            .arg(&move_path)
            .output()
            .context("Failed to run `sui client publish`. Is the Sui CLI installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "sui client publish failed:\n{}\n{}",
                stderr, stdout
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        let json_val: serde_json::Value = serde_json::from_str(&stdout)
            .context("Failed to parse sui publish JSON output")?;

        let mut package_id: Option<String> = None;
        let mut config_object_id: Option<String> = None;
        let mut cap_object_id: Option<String> = None;

        if let Some(changes) = json_val["objectChanges"].as_array() {
            for change in changes {
                let change_type = change["type"].as_str().unwrap_or("");
                match change_type {
                    "published" => {
                        package_id = change["packageId"].as_str().map(String::from);
                    }
                    "created" => {
                        let obj_type = change["objectType"].as_str().unwrap_or("");
                        let obj_id = change["objectId"].as_str().unwrap_or("");
                        if obj_type.contains("EnclaveConfig") {
                            config_object_id = Some(obj_id.to_string());
                        } else if obj_type.contains("Cap") {
                            cap_object_id = Some(obj_id.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        let pkg = package_id.context("Could not find package ID in publish output")?;
        let cfg = config_object_id.context("Could not find EnclaveConfig ID in publish output")?;
        let cap = cap_object_id.context("Could not find Cap ID in publish output")?;

        println!();
        println!("{}", "Deployment Successful".bold().green());
        println!("  {} Package ID:       {}", "▶".dimmed(), pkg.cyan());
        println!("  {} EnclaveConfig ID: {}", "▶".dimmed(), cfg.cyan());
        println!("  {} Cap ID:           {}", "▶".dimmed(), cap.cyan());

        // Save to .nautilus.toml
        let mut config = NautilusConfig::load(None).unwrap_or_default();
        config.sui.network = Some(args.network.clone());
        config.sui.package_id = Some(pkg);
        config.sui.config_object_id = Some(cfg);
        config.sui.cap_object_id = Some(cap);
        config.save(None)?;

        println!();
        println!(
            "{} Saved to .nautilus.toml. Future commands will use these IDs automatically.",
            "✔".green()
        );

        Ok(())
    }

    pub async fn register_enclave(args: RegisterEnclaveArgs) -> Result<()> {
        println!("{}", "Nautilus On-Chain Registration".bold().cyan());
        println!("{}", "─".repeat(40).dimmed());

        let config = NautilusConfig::load(None).unwrap_or_default();

        let package_id = resolve_id(&args.package_id, &config.sui.package_id, "package_id")?;
        let config_id = resolve_id(&args.config_object_id, &config.sui.config_object_id, "config_object_id")?;
        let cap_id = resolve_id(&args.cap_object_id, &config.sui.cap_object_id, "cap_object_id")?;

        // 0. If --pcr-file provided, update PCRs on-chain first
        if args.pcr_file.is_some() {
            println!("{} Updating on-chain PCRs first...", "→".cyan());
            let pcr_args = UpdatePcrsArgs {
                pcr_file: args.pcr_file.clone(),
                pcr0: None,
                pcr1: None,
                pcr2: None,
                network: args.network.clone(),
                package_id: Some(package_id.clone()),
                config_object_id: Some(config_id.clone()),
                cap_object_id: Some(cap_id.clone()),
                gas_budget: args.gas_budget,
            };
            update_pcrs(pcr_args).await?;
            println!();
        }

        // 1. Fetch attestation document from enclave
        let url = format!("http://{}:{}/get_attestation", args.host, args.port);
        println!("{} Fetching attestation from {}", "→".cyan(), url.cyan());

        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!(
                "Failed to connect to sign-server at {}.\n\
                 Ensure the enclave is running and the VSOCK bridge is active.",
                url
            ))?
            .json::<serde_json::Value>()
            .await
            .context("Failed to parse attestation response")?;

        let attestation_hex = resp["attestation"]
            .as_str()
            .context("Response missing 'attestation' field")?;

        // Validate it's valid hex
        let attestation_bytes = hex::decode(attestation_hex)
            .context("Attestation field is not valid hex")?;

        println!(
            "{} Got attestation document ({} bytes)",
            "✔".green(),
            attestation_bytes.len()
        );

        // 2. Build and execute PTB via `sui client ptb`
        // Chain two move calls:
        //   a) sui::nitro_attestation::load_nitro_attestation(attestation_bytes, clock) → doc
        //   b) nautilus::enclave::register_enclave<ENCLAVE>(config, cap, doc)
        println!("{} Submitting on-chain registration...", "→".cyan());

        let type_arg = format!("{}::enclave::ENCLAVE", package_id);
        let attestation_vec = format!("vector[{}]",
            attestation_bytes.iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let output = std::process::Command::new("sui")
            .args([
                "client", "ptb",
                "--json",
                "--move-call", "0x2::nitro_attestation::load_nitro_attestation",
                &attestation_vec, "@0x6",
                "--assign", "doc",
                "--move-call", &format!("{}::enclave::register_enclave", package_id),
                &format!("<{}>", type_arg),
                &format!("@{}", config_id), &format!("@{}", cap_id), "doc",
                "--gas-budget", &args.gas_budget.to_string(),
            ])
            .output()
            .context("Failed to run `sui client ptb`. Is the Sui CLI installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "sui client ptb failed:\n{}\n{}",
                stderr, stdout
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        println!();
        println!("{}", "Registration Successful".bold().green());

        // Try to extract digest and created objects from JSON output
        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(digest) = json_val["digest"].as_str() {
                println!("  {} Transaction: {}", "▶".dimmed(), digest.cyan());
            }
            if let Some(changes) = json_val["objectChanges"].as_array() {
                for change in changes {
                    let obj_type = change["objectType"].as_str().unwrap_or("");
                    if obj_type.contains("Enclave<") && !obj_type.contains("EnclaveConfig") {
                        if let Some(obj_id) = change["objectId"].as_str() {
                            println!("  {} Enclave ID:   {}", "▶".dimmed(), obj_id.cyan());
                        }
                    }
                }
            }
        }

        println!();
        println!(
            "{} Enclave is now registered on-chain. dApps can verify signatures using verify_signature().",
            "ℹ".bold().blue()
        );

        Ok(())
    }

    pub async fn update_pcrs(args: UpdatePcrsArgs) -> Result<()> {
        println!("{}", "Nautilus PCR Update".bold().cyan());
        println!("{}", "─".repeat(40).dimmed());

        let config = NautilusConfig::load(None).unwrap_or_default();

        let package_id = resolve_id(&args.package_id, &config.sui.package_id, "package_id")?;
        let config_id = resolve_id(&args.config_object_id, &config.sui.config_object_id, "config_object_id")?;
        let cap_id = resolve_id(&args.cap_object_id, &config.sui.cap_object_id, "cap_object_id")?;

        // Get PCR values from file or CLI args
        let (pcr0, pcr1, pcr2) = if let Some(ref pcr_file) = args.pcr_file {
            let content = std::fs::read_to_string(pcr_file)
                .with_context(|| format!("Failed to read PCR file: {}", pcr_file.display()))?;

            // Support both JSON format (from nautilus build) and plain text (from nitro.pcrs)
            if content.trim_start().starts_with('{') {
                let json: serde_json::Value = serde_json::from_str(&content)
                    .context("Failed to parse PCR JSON")?;
                let p0 = json["pcr0"].as_str()
                    .or_else(|| json["PCR0"].as_str())
                    .context("Missing pcr0/PCR0 in JSON")?
                    .to_string();
                let p1 = json["pcr1"].as_str()
                    .or_else(|| json["PCR1"].as_str())
                    .context("Missing pcr1/PCR1 in JSON")?
                    .to_string();
                let p2 = json["pcr2"].as_str()
                    .or_else(|| json["PCR2"].as_str())
                    .context("Missing pcr2/PCR2 in JSON")?
                    .to_string();
                (p0, p1, p2)
            } else {
                // Plain text format: "hash PCR0\nhash PCR1\nhash PCR2"
                let lines: Vec<&str> = content.lines().collect();
                if lines.len() < 3 {
                    anyhow::bail!("PCR file must have at least 3 lines (one per PCR)");
                }
                let p0 = lines[0].split_whitespace().next()
                    .context("Empty PCR0 line")?
                    .to_string();
                let p1 = lines[1].split_whitespace().next()
                    .context("Empty PCR1 line")?
                    .to_string();
                let p2 = lines[2].split_whitespace().next()
                    .context("Empty PCR2 line")?
                    .to_string();
                (p0, p1, p2)
            }
        } else {
            let p0 = args.pcr0.context("Provide --pcr-file or all of --pcr0, --pcr1, --pcr2")?;
            let p1 = args.pcr1.context("Missing --pcr1")?;
            let p2 = args.pcr2.context("Missing --pcr2")?;
            (p0, p1, p2)
        };

        // Validate hex
        let _pcr0_bytes = hex::decode(&pcr0).context("PCR0 is not valid hex")?;
        let _pcr1_bytes = hex::decode(&pcr1).context("PCR1 is not valid hex")?;
        let _pcr2_bytes = hex::decode(&pcr2).context("PCR2 is not valid hex")?;

        println!("  {} PCR0: {}...", "▶".dimmed(), &pcr0[..16.min(pcr0.len())].cyan());
        println!("  {} PCR1: {}...", "▶".dimmed(), &pcr1[..16.min(pcr1.len())].cyan());
        println!("  {} PCR2: {}...", "▶".dimmed(), &pcr2[..16.min(pcr2.len())].cyan());

        // Build and submit transaction via sui CLI
        let type_arg = format!("{}::enclave::ENCLAVE", package_id);

        let output = std::process::Command::new("sui")
            .args([
                "client", "call",
                "--json",
                "--package", &package_id,
                "--module", "enclave",
                "--function", "update_pcrs",
                "--type-args", &type_arg,
                "--args", &config_id, &cap_id,
                &format!("0x{}", pcr0),
                &format!("0x{}", pcr1),
                &format!("0x{}", pcr2),
                "--gas-budget", &args.gas_budget.to_string(),
            ])
            .output()
            .context("Failed to run `sui client call`. Is the Sui CLI installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!("sui client call failed:\n{}\n{}", stderr, stdout);
        }

        println!();
        println!("{}", "PCR Update Successful".bold().green());
        println!(
            "{} The EnclaveConfig now expects these PCR values. Previous enclave registration is invalidated.",
            "ℹ".bold().blue()
        );

        Ok(())
    }

    pub async fn verify_signature(args: VerifySignatureArgs) -> Result<()> {
        println!("{}", "Nautilus Signature Verification".bold().cyan());
        println!("{}", "─".repeat(40).dimmed());

        let config = NautilusConfig::load(None).unwrap_or_default();
        let package_id = resolve_id(&args.package_id, &config.sui.package_id, "package_id")?;

        // 1. Call /sign_name on the enclave
        let url = format!("http://{}:{}/sign_name", args.host, args.port);
        println!("{} Calling {} with name \"{}\"", "→".cyan(), url.cyan(), args.name);

        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(&url)
            .json(&serde_json::json!({ "name": args.name }))
            .send()
            .await
            .with_context(|| format!("Failed to connect to sign-server at {}", url))?
            .json::<serde_json::Value>()
            .await
            .context("Failed to parse sign_name response")?;

        let response = &resp["response"];
        let intent = response["intent"]
            .as_u64()
            .context("Missing 'intent' in response")?;
        let timestamp_ms = response["timestamp_ms"]
            .as_u64()
            .context("Missing 'timestamp_ms' in response")?;
        let name = response["data"]["name"]
            .as_str()
            .context("Missing 'data.name' in response")?;
        let message = response["data"]["message"]
            .as_str()
            .context("Missing 'data.message' in response")?;
        let signature_hex = resp["signature"]
            .as_str()
            .context("Missing 'signature' in response")?;

        // Validate hex
        hex::decode(signature_hex)
            .context("Signature is not valid hex")?;

        println!("{} Got signed response:", "✔".green());
        println!("  {} intent: {}", "▶".dimmed(), intent);
        println!("  {} timestamp_ms: {}", "▶".dimmed(), timestamp_ms);
        println!("  {} name: {}", "▶".dimmed(), name.cyan());
        println!("  {} signature: {}...", "▶".dimmed(), &signature_hex[..32]);

        // 2. Call verify_signed_name on-chain
        println!();
        println!("{} Verifying signature on-chain...", "→".cyan());

        let sig_vec = format!("0x{}", signature_hex);

        let output = std::process::Command::new("sui")
            .args([
                "client", "call",
                "--json",
                "--package", &package_id,
                "--module", "enclave",
                "--function", "verify_signed_name",
                "--args",
                &args.enclave_id,
                &intent.to_string(),
                &timestamp_ms.to_string(),
                name,
                message,
                &sig_vec,
                "--gas-budget", &args.gas_budget.to_string(),
            ])
            .output()
            .context("Failed to run `sui client call`")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!();
            println!("{}", "Signature Verification FAILED".bold().red());
            println!("  {} The on-chain verify_signed_name aborted.", "✗".red());
            println!("  {} This means the signature does not match the enclave's public key.", "ℹ".blue());
            anyhow::bail!(
                "verify_signed_name failed:\n{}\n{}",
                stderr, stdout
            );
        }

        let stdout_str = String::from_utf8_lossy(&output.stdout);

        println!();
        println!("{}", "Signature Verification PASSED".bold().green());
        println!("  {} The signature was verified on-chain against the registered enclave.", "✔".green());

        // Extract digest from JSON output
        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&stdout_str) {
            if let Some(digest) = json_val["digest"].as_str() {
                println!("  {} Transaction: {}", "▶".dimmed(), digest.cyan());
            }
        }

        println!();
        println!(
            "{} The enclave's Ed25519 signature matches the on-chain public key from attestation.",
            "ℹ".bold().blue()
        );

        Ok(())
    }
}

// ─── Stubs when sui feature is disabled ─────────────────────────────────

#[cfg(not(feature = "sui"))]
mod implementation {
    use super::*;

    pub async fn deploy_contract(_args: DeployContractArgs) -> Result<()> {
        anyhow::bail!(
            "Sui support is not compiled in.\n\
             Rebuild with: cargo build --features sui"
        );
    }

    pub async fn register_enclave(_args: RegisterEnclaveArgs) -> Result<()> {
        anyhow::bail!(
            "Sui support is not compiled in.\n\
             Rebuild with: cargo build --features sui"
        );
    }

    pub async fn update_pcrs(_args: UpdatePcrsArgs) -> Result<()> {
        anyhow::bail!(
            "Sui support is not compiled in.\n\
             Rebuild with: cargo build --features sui"
        );
    }

    pub async fn verify_signature(_args: VerifySignatureArgs) -> Result<()> {
        anyhow::bail!(
            "Sui support is not compiled in.\n\
             Rebuild with: cargo build --features sui"
        );
    }
}

// Re-export the implementation functions
pub use implementation::*;
