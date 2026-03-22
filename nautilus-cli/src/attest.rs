use anyhow::{Context, Result};
use ciborium::value::Value as CborValue;
use clap::Args;
use colored::Colorize;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::{self, Template};

#[derive(Args, Debug)]
pub struct AttestArgs {
    /// EC2 instance public hostname or IP (must be reachable from your machine).
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// TCP port. Rust default: 4000 (socat bridge). TS default: 3000 (argonaut bridge).
    #[arg(long)]
    pub port: Option<u16>,

    /// Write the PCRs + attestation to a JSON file.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn run(args: AttestArgs, cli_template: Option<Template>) -> Result<()> {
    let cfg = config::NautilusConfig::load(None).unwrap_or_default();
    let template = config::resolve_template(cli_template, &cfg)?;

    println!("{}", "Nautilus Attestation Client".bold().cyan());
    println!(
        "{} Template: {}",
        "→".cyan(),
        template.to_string().cyan().bold()
    );
    println!("{}", "─".repeat(40).dimmed());

    match template {
        Template::Rust => run_rust_attest(args).await,
        Template::Ts | Template::Python => run_ts_attest(args, template).await,
    }
}

/// Rust template: HTTP GET /get_attestation (port 4000 default).
async fn run_rust_attest(args: AttestArgs) -> Result<()> {
    let port = args.port.unwrap_or(4000);
    let url = format!("http://{}:{}/get_attestation", args.host, port);
    println!("{} Fetching attestation from {}", "→".cyan(), url.cyan());

    let addr = format!("{}:{}", args.host, port);
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!(
            "Cannot reach enclave at {}.\n\
             Ensure the enclave is running and the socat bridge is active.",
            addr
        ))?;

    stream.set_read_timeout(Some(Duration::from_secs(15)))?;

    let http_req = format!(
        "GET /get_attestation HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        args.host, port
    );
    stream.write_all(http_req.as_bytes())?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response_str = String::from_utf8_lossy(&response);

    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .context("Invalid HTTP response — no body found")?;

    let json: serde_json::Value = serde_json::from_str(body)
        .with_context(|| format!("Failed to parse attestation JSON: {}", &body[..body.len().min(200)]))?;

    let attestation_hex = json["attestation"]
        .as_str()
        .context("Response missing 'attestation' field")?;

    let attestation_bytes = hex::decode(attestation_hex)
        .context("Attestation field is not valid hex")?;

    println!(
        "{} Got attestation document ({} bytes)",
        "✔".green(),
        attestation_bytes.len()
    );

    let parsed = parse_attestation_cbor(&attestation_bytes)?;

    println!();
    println!("{}", "Attestation Document".bold().yellow());
    println!("  {} Public Key: {}", "▶".dimmed(), parsed.public_key.cyan());
    println!();
    println!("{}", "PCR Measurements (from NSM)".bold().yellow());
    println!("  {} PCR0: {}", "▶".dimmed(), parsed.pcr0.cyan());
    println!("  {} PCR1: {}", "▶".dimmed(), parsed.pcr1.cyan());
    println!("  {} PCR2: {}", "▶".dimmed(), parsed.pcr2.cyan());

    if let Some(ref out_path) = args.out {
        let pcrs_json = serde_json::json!({
            "pcr0": parsed.pcr0,
            "pcr1": parsed.pcr1,
            "pcr2": parsed.pcr2,
            "public_key": parsed.public_key,
            "raw_cbor_hex": attestation_hex,
        });
        std::fs::write(out_path, serde_json::to_string_pretty(&pcrs_json)?)
            .with_context(|| format!("Failed to write to {}", out_path.display()))?;
        println!();
        println!(
            "{} PCRs + attestation written to: {}",
            "✔".green(),
            out_path.display()
        );
    }

    println!();
    println!(
        "{} Next: update PCRs on-chain, then register this enclave.",
        "ℹ".bold().blue()
    );

    Ok(())
}

/// TS/Python template: HTTP GET /attestation (port from template default).
/// Uses a raw TCP HTTP/1.1 request to avoid requiring reqwest without `sui` feature.
async fn run_ts_attest(args: AttestArgs, template: Template) -> Result<()> {
    let port = args.port.unwrap_or(template.default_http_port());
    let path = template.attestation_path();
    let url = format!("http://{}:{}{}", args.host, port, path);
    println!("{} Fetching attestation from {}", "→".cyan(), url.cyan());

    let addr = format!("{}:{}", args.host, port);
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!(
            "Cannot reach enclave at {}.\n\
             Ensure the enclave is running and the bridge is active.",
            addr
        ))?;

    stream.set_read_timeout(Some(Duration::from_secs(15)))?;

    let http_req = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        path, args.host, port
    );
    stream.write_all(http_req.as_bytes())?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response_str = String::from_utf8_lossy(&response);

    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .context("Invalid HTTP response — no body found")?;

    let json: serde_json::Value = serde_json::from_str(body)
        .with_context(|| format!("Failed to parse attestation JSON: {}", &body[..body.len().min(200)]))?;

    let attestation_hex = json["attestation"]
        .as_str()
        .context("Response missing 'attestation' field")?;

    let attestation_bytes = hex::decode(attestation_hex)
        .context("Attestation field is not valid hex")?;

    println!(
        "{} Got attestation document ({} bytes)",
        "✔".green(),
        attestation_bytes.len()
    );

    // Parse COSE_Sign1 → extract payload → extract PCRs + public_key
    let parsed = parse_attestation_cbor(&attestation_bytes)?;

    println!();
    println!("{}", "Attestation Document".bold().yellow());
    println!("  {} Public Key: {}", "▶".dimmed(), parsed.public_key.cyan());
    println!();
    println!("{}", "PCR Measurements (from NSM)".bold().yellow());
    println!("  {} PCR0: {}", "▶".dimmed(), parsed.pcr0.cyan());
    println!("  {} PCR1: {}", "▶".dimmed(), parsed.pcr1.cyan());
    println!("  {} PCR2: {}", "▶".dimmed(), parsed.pcr2.cyan());

    if let Some(ref out_path) = args.out {
        let pcrs_json = serde_json::json!({
            "pcr0": parsed.pcr0,
            "pcr1": parsed.pcr1,
            "pcr2": parsed.pcr2,
            "public_key": parsed.public_key,
            "raw_cbor_hex": attestation_hex,
        });
        std::fs::write(out_path, serde_json::to_string_pretty(&pcrs_json)?)
            .with_context(|| format!("Failed to write to {}", out_path.display()))?;
        println!();
        println!(
            "{} PCRs + attestation written to: {}",
            "✔".green(),
            out_path.display()
        );
    }

    println!();
    println!(
        "{} Next: update PCRs on-chain, then register this enclave.",
        "ℹ".bold().blue()
    );

    Ok(())
}

/// Parsed fields from a Nitro attestation COSE_Sign1 document.
struct ParsedAttestation {
    pcr0: String,
    pcr1: String,
    pcr2: String,
    public_key: String,
}

/// Parse a COSE_Sign1 attestation document (CBOR) to extract PCRs and public key.
///
/// Structure: CBOR Tag(18) or Array [protected, unprotected, payload, signature]
/// Payload is a CBOR map containing "pcrs" (map {0: bytes, 1: bytes, ...})
/// and "public_key" (bytes).
fn parse_attestation_cbor(data: &[u8]) -> Result<ParsedAttestation> {
    let cose: CborValue = ciborium::from_reader(data)
        .context("Failed to parse COSE_Sign1 CBOR")?;

    // COSE_Sign1 is a CBOR array of 4 elements; may be wrapped in Tag(18)
    let arr = match &cose {
        CborValue::Tag(18, inner) => match inner.as_ref() {
            CborValue::Array(a) => a,
            _ => anyhow::bail!("COSE_Sign1 tag(18) does not contain an array"),
        },
        CborValue::Array(a) => a,
        _ => anyhow::bail!("Expected COSE_Sign1 array, got {:?}", cose),
    };

    if arr.len() < 4 {
        anyhow::bail!("COSE_Sign1 array has {} elements, expected 4", arr.len());
    }

    // Element [2] is the payload (bstr containing a CBOR-encoded map)
    let payload_bytes = match &arr[2] {
        CborValue::Bytes(b) => b,
        _ => anyhow::bail!("COSE_Sign1 payload is not a byte string"),
    };

    let payload: CborValue = ciborium::from_reader(payload_bytes.as_slice())
        .context("Failed to parse attestation payload CBOR")?;

    let payload_map = match &payload {
        CborValue::Map(m) => m,
        _ => anyhow::bail!("Attestation payload is not a CBOR map"),
    };

    // Extract PCRs: key "pcrs" → map { Integer(0): Bytes, Integer(1): Bytes, ... }
    let pcrs_map = payload_map.iter()
        .find(|(k, _)| matches!(k, CborValue::Text(s) if s == "pcrs"))
        .map(|(_, v)| v)
        .context("Attestation payload missing 'pcrs' field")?;

    let pcrs = match pcrs_map {
        CborValue::Map(m) => m,
        _ => anyhow::bail!("'pcrs' field is not a CBOR map"),
    };

    let extract_pcr = |index: i128| -> Result<String> {
        pcrs.iter()
            .find(|(k, _)| matches!(k, CborValue::Integer(i) if i128::from(*i) == index))
            .and_then(|(_, v)| match v {
                CborValue::Bytes(b) => Some(hex::encode(b)),
                _ => None,
            })
            .with_context(|| format!("Missing or invalid PCR{}", index))
    };

    let pcr0 = extract_pcr(0)?;
    let pcr1 = extract_pcr(1)?;
    let pcr2 = extract_pcr(2)?;

    // Extract public_key: key "public_key" → Bytes
    let public_key = payload_map.iter()
        .find(|(k, _)| matches!(k, CborValue::Text(s) if s == "public_key"))
        .and_then(|(_, v)| match v {
            CborValue::Bytes(b) => Some(hex::encode(b)),
            _ => None,
        })
        .unwrap_or_default();

    Ok(ParsedAttestation { pcr0, pcr1, pcr2, public_key })
}

