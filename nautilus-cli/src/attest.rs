use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::config::{self, Template};

/// Command sent over the host-side TCP-to-VSOCK proxy connection (Rust template only).
const CMD_GET_ATTESTATION: u8 = 0x01;

#[derive(Args, Debug)]
pub struct AttestArgs {
    /// EC2 instance public hostname or IP (must be reachable from your machine).
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// TCP port. Rust default: 5000 (sidecar binary protocol). TS default: 3000 (HTTP).
    #[arg(long)]
    pub port: Option<u16>,

    /// Optional nonce (hex) to embed in the attestation user-data field (Rust template only).
    #[arg(long)]
    pub nonce: Option<String>,

    /// Write the raw CBOR attestation document to a file for offline inspection.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// Minimal subset of an NSM attestation doc that we can display without a full
/// CBOR/COSE parser on the client side. The sidecar annotates the document
/// with a JSON envelope for developer convenience.
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationEnvelope {
    /// Hex-encoded PCR0 extracted by the sidecar from the NSM response.
    pub pcr0: String,
    /// Hex-encoded PCR1.
    pub pcr1: String,
    /// Hex-encoded PCR2.
    pub pcr2: String,
    /// Hex-encoded Ed25519 public key embedded as NSM user-data.
    pub public_key: String,
    /// ISO-8601 timestamp set by the sidecar at the moment of request.
    pub timestamp: String,
    /// Raw CBOR bytes of the full COSE_Sign1 attestation document (hex).
    pub raw_cbor_hex: String,
}

pub async fn run(args: AttestArgs) -> Result<()> {
    let cfg = config::NautilusConfig::load(None).unwrap_or_default();
    let template = config::resolve_template(None, &cfg)?;

    println!("{}", "Nautilus Attestation Client".bold().cyan());
    println!(
        "{} Template: {}",
        "→".cyan(),
        template.to_string().cyan().bold()
    );
    println!("{}", "─".repeat(40).dimmed());

    match template {
        Template::Rust => run_rust_attest(args).await,
        Template::Ts => run_ts_attest(args).await,
    }
}

/// Rust template: binary sidecar protocol over TCP (port 5000 default).
async fn run_rust_attest(args: AttestArgs) -> Result<()> {
    let port = args.port.unwrap_or(5000);
    let addr = format!("{}:{}", args.host, port);
    println!("{} Connecting to sidecar at {}", "→".cyan(), addr.cyan());

    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!(
            "Cannot reach enclave VSOCK proxy at {}.\n\
             Ensure the EC2 host is running and port {} is open (security group + socat forwarder).",
            addr, port
        ))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .context("Failed to set read timeout")?;

    let nonce_bytes: Vec<u8> = match args.nonce {
        Some(ref hex) => hex::decode(hex)
            .context("--nonce must be a valid hex string")?,
        None => random_nonce(),
    };

    let mut req = vec![CMD_GET_ATTESTATION];
    req.extend_from_slice(&(nonce_bytes.len() as u16).to_le_bytes());
    req.extend_from_slice(&nonce_bytes);

    stream
        .write_all(&req)
        .context("Failed to send attestation request to enclave proxy")?;

    println!("{} Request sent (nonce: {} bytes)", "✔".green(), nonce_bytes.len());

    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .context("Failed to read response length from enclave")?;

    let payload_len = u32::from_le_bytes(len_buf) as usize;
    if payload_len == 0 || payload_len > 64 * 1024 {
        anyhow::bail!(
            "Unexpected response length {} from enclave. \
             This may indicate a protocol version mismatch.",
            payload_len
        );
    }

    let mut payload = vec![0u8; payload_len];
    stream
        .read_exact(&mut payload)
        .context("Failed to read response payload from enclave")?;

    let envelope: AttestationEnvelope = serde_json::from_slice(&payload)
        .context("Failed to parse attestation envelope.")?;

    println!();
    println!("{}", "Attestation Document".bold().yellow());
    println!("  {} Timestamp:  {}", "▶".dimmed(), envelope.timestamp.cyan());
    println!("  {} Public Key: {}", "▶".dimmed(), envelope.public_key.cyan());
    println!();
    println!("{}", "PCR Measurements (from NSM)".bold().yellow());
    println!("  {} PCR0: {}", "▶".dimmed(), envelope.pcr0.cyan());
    println!("  {} PCR1: {}", "▶".dimmed(), envelope.pcr1.cyan());
    println!("  {} PCR2: {}", "▶".dimmed(), envelope.pcr2.cyan());

    if let Some(ref out_path) = args.out {
        let raw = hex::decode(&envelope.raw_cbor_hex)
            .context("Sidecar returned invalid hex for raw_cbor_hex")?;
        std::fs::write(out_path, &raw)
            .with_context(|| format!("Failed to write attestation doc to {}", out_path.display()))?;
        println!();
        println!(
            "{} Raw CBOR written to: {}",
            "✔".green(),
            out_path.display()
        );
    }

    println!();
    println!(
        "{} Next: register this enclave on Sui.",
        "ℹ".bold().blue()
    );

    Ok(())
}

/// TS template: HTTP GET /attestation (port 3000 default).
/// Uses a raw TCP HTTP/1.1 request to avoid requiring reqwest without `sui` feature.
async fn run_ts_attest(args: AttestArgs) -> Result<()> {
    let port = args.port.unwrap_or(3000);
    let url = format!("http://{}:{}/attestation", args.host, port);
    println!("{} Fetching attestation from {}", "→".cyan(), url.cyan());

    let addr = format!("{}:{}", args.host, port);
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!(
            "Cannot reach enclave at {}.\n\
             Ensure the enclave is running and the argonaut bridge is active.",
            addr
        ))?;

    stream.set_read_timeout(Some(Duration::from_secs(15)))?;

    // Send a minimal HTTP/1.1 GET request
    let http_req = format!(
        "GET /attestation HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        args.host, port
    );
    stream.write_all(http_req.as_bytes())?;

    // Read the full response
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response_str = String::from_utf8_lossy(&response);

    // Split headers from body (separated by \r\n\r\n)
    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .context("Invalid HTTP response — no body found")?;

    // Parse JSON: {"attestation": "<hex>"}
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
    println!();
    println!("{}", "Attestation Document".bold().yellow());
    println!(
        "  {} CBOR hex: {}...",
        "▶".dimmed(),
        &attestation_hex[..attestation_hex.len().min(80)].cyan()
    );

    if let Some(ref out_path) = args.out {
        std::fs::write(out_path, &attestation_bytes)
            .with_context(|| format!("Failed to write attestation doc to {}", out_path.display()))?;
        println!();
        println!(
            "{} Raw CBOR written to: {}",
            "✔".green(),
            out_path.display()
        );
    }

    println!();
    println!(
        "{} Next: register this enclave on Sui.",
        "ℹ".bold().blue()
    );

    Ok(())
}

fn random_nonce() -> Vec<u8> {
    // Simple PRNG-free nonce using current time + stack address entropy.
    // For production use, the caller should pass a nonce from a CSPRNG.
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut buf = [0u8; 32];
    let bytes = t.to_le_bytes();
    buf[..16].copy_from_slice(&bytes);
    // XOR the upper half with the lower to add some bit variation
    for i in 0..16 {
        buf[16 + i] = bytes[i] ^ 0xA5u8.wrapping_add(i as u8);
    }
    buf.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_nonce_length() {
        let n = random_nonce();
        assert_eq!(n.len(), 32);
    }

    #[test]
    fn test_random_nonce_not_all_zeros() {
        let n = random_nonce();
        assert!(n.iter().any(|&b| b != 0));
    }
}
