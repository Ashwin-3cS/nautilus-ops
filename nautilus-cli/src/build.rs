use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// PCR (Platform Configuration Register) measurements extracted from a built EIF.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct PcrMeasurements {
    #[serde(rename = "PCR0")]
    pub pcr0: String,
    #[serde(rename = "PCR1")]
    pub pcr1: String,
    #[serde(rename = "PCR2")]
    pub pcr2: String,
    #[serde(rename = "PCR8")]
    pub pcr8: Option<String>, // only present when signed
}

/// Raw JSON shape that nitro-cli outputs on a successful build.
#[derive(Debug, Deserialize)]
struct NitroCLIOutput {
    #[serde(rename = "Measurements")]
    measurements: PcrMeasurements,
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Path to the Dockerfile (or Containerfile) to build.
    #[arg(short = 'f', long, default_value = "Dockerfile")]
    pub dockerfile: PathBuf,

    /// Directory to use as the Docker build context (defaults to current dir).
    #[arg(short = 'C', long, default_value = ".")]
    pub context: PathBuf,

    /// Output path for the produced .eif file.
    #[arg(short, long, default_value = "out/enclave.eif")]
    pub output: PathBuf,

    /// Docker image tag to use as an intermediate build step.
    #[arg(long, default_value = "nautilus/enclave:build")]
    pub tag: String,

    /// Path to write the PCR JSON file (defaults to <output>.pcrs.json).
    #[arg(long)]
    pub pcr_out: Option<PathBuf>,
}

pub async fn run(args: BuildArgs) -> Result<()> {
    println!("{}", "Nautilus Build Engine".bold().cyan());
    println!("{}", "─".repeat(40).dimmed());

    // ------------------------------------------------------------------
    // Step 1: docker build
    // ------------------------------------------------------------------
    println!(
        "{} Building Docker image from {}",
        "→".cyan(),
        args.dockerfile.display()
    );

    let pb = spinner("Building Docker image…");
    let docker_status = Command::new("docker")
        .args([
            "build",
            "--tag",
            &args.tag,
            "--platform",
            "linux/amd64",
            "--progress=plain",
            "-f",
            args.dockerfile.to_str().context("Invalid Dockerfile path")?,
            args.context.to_str().context("Invalid context path")?,
        ])
        .status()
        .context("Failed to run `docker build`. Is Docker installed and running?")?;
    pb.finish_and_clear();

    if !docker_status.success() {
        anyhow::bail!("docker build failed with exit code {:?}", docker_status.code());
    }
    println!("  {} Docker image built: {}", "✔".green(), args.tag.cyan());

    // ------------------------------------------------------------------
    // Step 2: nitro-cli build-enclave
    // ------------------------------------------------------------------
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)
            .context("Failed to create output directory for .eif")?;
    }

    println!("{} Building Nitro Enclave Image (.eif)…", "→".cyan());
    let pb = spinner("Running nitro-cli build-enclave…");

    let nitro_output = Command::new("nitro-cli")
        .args([
            "build-enclave",
            "--docker-uri",
            &args.tag,
            "--output-file",
            args.output.to_str().context("Invalid output path")?,
        ])
        .output()
        .context(
            "Failed to run `nitro-cli build-enclave`. \
             Is nitro-cli installed? (sudo amazon-linux-extras install aws-nitro-enclaves-cli)",
        )?;
    pb.finish_and_clear();

    if !nitro_output.status.success() {
        let stderr = String::from_utf8_lossy(&nitro_output.stderr);
        anyhow::bail!("nitro-cli build-enclave failed:\n{}", stderr);
    }

    // ------------------------------------------------------------------
    // Step 3: Parse PCR measurements from nitro-cli JSON output
    // ------------------------------------------------------------------
    let stdout = String::from_utf8_lossy(&nitro_output.stdout);
    let pcrs = parse_pcrs_from_output(&stdout).context(
        "Failed to parse PCR measurements from nitro-cli output. \
         Ensure you are using a compatible version of nitro-cli.",
    )?;

    println!("{} Enclave image built: {}", "✔".green(), args.output.display());
    println!();
    println!("{}", "PCR Measurements".bold().yellow());
    println!("  {} PCR0 (kernel + boot): {}", "▶".dimmed(), pcrs.pcr0.cyan());
    println!("  {} PCR1 (kernel + OS):   {}", "▶".dimmed(), pcrs.pcr1.cyan());
    println!("  {} PCR2 (app image):     {}", "▶".dimmed(), pcrs.pcr2.cyan());
    if let Some(ref pcr8) = pcrs.pcr8 {
        println!("  {} PCR8 (signing cert):  {}", "▶".dimmed(), pcr8.cyan());
    }

    // ------------------------------------------------------------------
    // Step 4: Write PCR JSON sidecar
    // ------------------------------------------------------------------
    let pcr_path = args.pcr_out.unwrap_or_else(|| {
        let mut p = args.output.clone();
        p.set_extension("pcrs.json");
        p
    });

    let pcr_json = serde_json::to_string_pretty(&pcrs)?;
    std::fs::write(&pcr_path, &pcr_json)
        .with_context(|| format!("Failed to write PCR file to {}", pcr_path.display()))?;

    println!();
    println!(
        "{} PCR measurements saved to: {}",
        "✔".green(),
        pcr_path.display()
    );
    println!();
    println!(
        "{} Next: run {} to scaffold your CI/CD pipeline.",
        "ℹ".bold().blue(),
        "nautilus init-ci".bold()
    );

    Ok(())
}

/// Parses PCR measurements from nitro-cli JSON stdout.
/// nitro-cli outputs something like:
/// Start building the Enclave Image...
/// {"Measurements":{"PCR0":"abc...","PCR1":"def...","PCR2":"ghi..."}}
pub fn parse_pcrs_from_output(output: &str) -> Result<PcrMeasurements> {
    // Find the JSON line — nitro-cli intermixes prose with JSON
    let json_line = output
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .context("No JSON object found in nitro-cli output")?;

    let parsed: NitroCLIOutput = serde_json::from_str(json_line)
        .with_context(|| format!("Failed to parse nitro-cli JSON: {}", json_line))?;

    Ok(parsed.measurements)
}

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

// ─────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_NITRO_OUTPUT: &str = r#"
Start building the Enclave Image...
Enclave Image successfully created.
{"Measurements":{"PCR0":"aabbcc00112233445566778899aabbcc00112233445566778899aabbcc001122334455667788990011223344556677","PCR1":"bbccdd00112233445566778899aabbcc00112233445566778899aabbcc001122334455667788990011223344556677","PCR2":"ccdde00112233445566778899aabbcc00112233445566778899aabbcc001122334455667788990011223344556677"}}
"#;

    #[test]
    fn test_parse_pcrs_success() {
        let pcrs = parse_pcrs_from_output(SAMPLE_NITRO_OUTPUT).unwrap();
        assert!(pcrs.pcr0.starts_with("aabbcc"));
        assert!(pcrs.pcr1.starts_with("bbccdd"));
        assert!(pcrs.pcr2.starts_with("ccdd"));
        assert!(pcrs.pcr8.is_none());
    }

    #[test]
    fn test_parse_pcrs_with_pcr8() {
        let out = r#"
Start building the Enclave Image...
{"Measurements":{"PCR0":"aaa","PCR1":"bbb","PCR2":"ccc","PCR8":"ddd"}}
"#;
        let pcrs = parse_pcrs_from_output(out).unwrap();
        assert_eq!(pcrs.pcr8, Some("ddd".to_string()));
    }

    #[test]
    fn test_parse_pcrs_no_json_fails() {
        let result = parse_pcrs_from_output("This is just prose output with no JSON.");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_pcrs_invalid_json_fails() {
        let result = parse_pcrs_from_output("{invalid json}");
        assert!(result.is_err());
    }

    #[test]
    fn test_pcr_measurements_serialization_roundtrip() {
        let pcrs = PcrMeasurements {
            pcr0: "aaa".into(),
            pcr1: "bbb".into(),
            pcr2: "ccc".into(),
            pcr8: Some("ddd".into()),
        };
        let json = serde_json::to_string(&pcrs).unwrap();
        let back: PcrMeasurements = serde_json::from_str(&json).unwrap();
        assert_eq!(pcrs, back);
    }
}
