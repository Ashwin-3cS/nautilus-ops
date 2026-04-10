use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::config::{self, Template};

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

pub async fn run(args: BuildArgs, cli_template: Option<Template>) -> Result<()> {
    let cfg = config::NautilusConfig::load(None).unwrap_or_default();
    let template = config::resolve_template(cli_template, &cfg)?;

    println!("{}", "Nautilus Build Engine".bold().cyan());
    println!(
        "{} Template: {}",
        "→".cyan(),
        template.to_string().cyan().bold()
    );
    println!("{}", "─".repeat(40).dimmed());

    match template {
        Template::Rust | Template::Python | Template::MessagingRelayer | Template::MemwalRelayer => run_rust_build(args).await,
        Template::Ts => run_ts_build(args).await,
    }
}

async fn run_rust_build(args: BuildArgs) -> Result<()> {
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

    print_pcrs(&pcrs, &args.output);
    write_pcrs_json(&pcrs, &args.output, args.pcr_out)?;

    Ok(())
}

async fn run_ts_build(args: BuildArgs) -> Result<()> {
    let context = args.context.canonicalize().unwrap_or_else(|_| args.context.clone());

    // Check for Makefile
    if !context.join("Makefile").exists() {
        anyhow::bail!(
            "No Makefile found in {}. nautilus-ts projects require a Makefile with an EIF build target.",
            context.display()
        );
    }

    // ------------------------------------------------------------------
    // Step 1: Run `make` in the project directory
    // ------------------------------------------------------------------
    println!(
        "{} Running make in {}",
        "→".cyan(),
        context.display()
    );

    let pb = spinner("Building EIF via make (docker multi-stage)…");
    let make_status = Command::new("make")
        .current_dir(&context)
        .status()
        .context("Failed to run `make`. Is make installed?")?;
    pb.finish_and_clear();

    if !make_status.success() {
        anyhow::bail!("make failed with exit code {:?}", make_status.code());
    }

    // ------------------------------------------------------------------
    // Step 2: Locate and parse PCR measurements
    // ------------------------------------------------------------------
    let eif_path = context.join("out/nitro.eif");
    let pcrs_path = context.join("out/nitro.pcrs");

    if !eif_path.exists() {
        anyhow::bail!(
            "EIF not found at {}. Expected make to produce out/nitro.eif",
            eif_path.display()
        );
    }

    println!("{} Enclave image built: {}", "✔".green(), eif_path.display());

    if pcrs_path.exists() {
        let content = std::fs::read_to_string(&pcrs_path)
            .with_context(|| format!("Failed to read {}", pcrs_path.display()))?;
        let pcrs = parse_pcrs_from_plain_text(&content)?;
        print_pcrs(&pcrs, &eif_path);
        write_pcrs_json(&pcrs, &eif_path, args.pcr_out)?;
    } else {
        println!(
            "  {} PCR file not found at {}. Skipping PCR output.",
            "⚠".yellow(),
            pcrs_path.display()
        );
    }

    // Check for argonaut binary (needed on host side)
    let argonaut_path = context.join("out/argonaut");
    if argonaut_path.exists() {
        println!(
            "{} Host binary built: {}",
            "✔".green(),
            argonaut_path.display()
        );
    }

    println!();
    println!(
        "{} Next: run {} to scaffold your CI/CD pipeline.",
        "ℹ".bold().blue(),
        "nautilus init-ci --template ts".bold()
    );

    Ok(())
}

fn print_pcrs(pcrs: &PcrMeasurements, eif_path: &PathBuf) {
    println!("{} Enclave image: {}", "✔".green(), eif_path.display());
    println!();
    println!("{}", "PCR Measurements".bold().yellow());
    println!("  {} PCR0 (kernel + boot): {}", "▶".dimmed(), pcrs.pcr0.cyan());
    println!("  {} PCR1 (kernel + OS):   {}", "▶".dimmed(), pcrs.pcr1.cyan());
    println!("  {} PCR2 (app image):     {}", "▶".dimmed(), pcrs.pcr2.cyan());
    if let Some(ref pcr8) = pcrs.pcr8 {
        println!("  {} PCR8 (signing cert):  {}", "▶".dimmed(), pcr8.cyan());
    }
}

fn write_pcrs_json(
    pcrs: &PcrMeasurements,
    eif_path: &PathBuf,
    pcr_out: Option<PathBuf>,
) -> Result<()> {
    let pcr_path = pcr_out.unwrap_or_else(|| {
        let mut p = eif_path.clone();
        p.set_extension("pcrs.json");
        p
    });

    let pcr_json = serde_json::to_string_pretty(pcrs)?;
    std::fs::write(&pcr_path, &pcr_json)
        .with_context(|| format!("Failed to write PCR file to {}", pcr_path.display()))?;

    println!();
    println!(
        "{} PCR measurements saved to: {}",
        "✔".green(),
        pcr_path.display()
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

/// Parses PCR measurements from a plain text file (nautilus-ts `out/nitro.pcrs`).
/// Format: one hash per line, e.g.:
///   aabbcc... PCR0
///   ddeeff... PCR1
///   112233... PCR2
/// or just three hash lines without labels.
pub fn parse_pcrs_from_plain_text(content: &str) -> Result<PcrMeasurements> {
    let lines: Vec<&str> = content.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.len() < 3 {
        anyhow::bail!(
            "PCR plain text file must have at least 3 lines (one per PCR), found {}",
            lines.len()
        );
    }

    // Each line is either "hash" or "hash LABEL" — take the first whitespace-delimited token
    let pcr0 = lines[0].split_whitespace().next()
        .context("Empty PCR0 line")?.to_string();
    let pcr1 = lines[1].split_whitespace().next()
        .context("Empty PCR1 line")?.to_string();
    let pcr2 = lines[2].split_whitespace().next()
        .context("Empty PCR2 line")?.to_string();

    Ok(PcrMeasurements {
        pcr0,
        pcr1,
        pcr2,
        pcr8: None,
    })
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

    #[test]
    fn test_parse_pcrs_from_plain_text() {
        let content = "aabbcc0011 PCR0\nddeeff2233 PCR1\n112233aabb PCR2\n";
        let pcrs = parse_pcrs_from_plain_text(content).unwrap();
        assert_eq!(pcrs.pcr0, "aabbcc0011");
        assert_eq!(pcrs.pcr1, "ddeeff2233");
        assert_eq!(pcrs.pcr2, "112233aabb");
        assert!(pcrs.pcr8.is_none());
    }

    #[test]
    fn test_parse_pcrs_from_plain_text_no_labels() {
        let content = "aabbcc0011\nddeeff2233\n112233aabb\n";
        let pcrs = parse_pcrs_from_plain_text(content).unwrap();
        assert_eq!(pcrs.pcr0, "aabbcc0011");
        assert_eq!(pcrs.pcr1, "ddeeff2233");
        assert_eq!(pcrs.pcr2, "112233aabb");
    }

    #[test]
    fn test_parse_pcrs_from_plain_text_too_few_lines() {
        let content = "aabbcc0011\nddeeff2233\n";
        assert!(parse_pcrs_from_plain_text(content).is_err());
    }

    #[test]
    fn test_parse_pcrs_from_plain_text_with_empty_lines() {
        let content = "\naabbcc0011\n\nddeeff2233\n\n112233aabb\n";
        let pcrs = parse_pcrs_from_plain_text(content).unwrap();
        assert_eq!(pcrs.pcr0, "aabbcc0011");
        assert_eq!(pcrs.pcr1, "ddeeff2233");
        assert_eq!(pcrs.pcr2, "112233aabb");
    }
}
