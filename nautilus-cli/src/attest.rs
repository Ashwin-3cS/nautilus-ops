use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

/// Command sent over the host-side TCP-to-VSOCK proxy connection.
/// The EC2 host runs a small forwarder that exposes the enclave VSOCK on a
/// local TCP port via `socat VSOCK-LISTEN:5000,fork TCP4-CONNECT:127.0.0.1:15000`
/// (or equivalent). This avoids needing raw VSOCK support on the developer
/// machine while keeping the sidecar protocol unchanged.
const CMD_GET_ATTESTATION: u8 = 0x01;

/// The sidecar returns length-prefixed CBOR. For the `attest` sub-command we
/// tell the sidecar to include a well-known nonce so the attestation doc is
/// unique for each invocation.
#[derive(Args, Debug)]
pub struct AttestArgs {
    /// EC2 instance public hostname or IP (must be reachable from your machine).
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// TCP port that is forwarded to the enclave VSOCK on the EC2 host.
    /// Default matches the sidecar's VSOCK port (5000) exposed as TCP.
    #[arg(long, default_value = "5000")]
    pub port: u16,

    /// Optional nonce (hex) to embed in the attestation user-data field.
    /// A random 32-byte nonce is used if not provided.
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
    println!("{}", "Nautilus Attestation Client".bold().cyan());
    println!("{}", "─".repeat(40).dimmed());

    let addr = format!("{}:{}", args.host, args.port);
    println!(
        "{} Connecting to enclave proxy at {}",
        "→".cyan(),
        addr.cyan()
    );

    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!(
            "Cannot reach enclave VSOCK proxy at {}.\n\
             Ensure the EC2 host is running and port {} is open (security group + socat forwarder).",
            addr, args.port
        ))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .context("Failed to set read timeout")?;

    // ── Build request ────────────────────────────────────────────────────
    let nonce_bytes: Vec<u8> = match args.nonce {
        Some(ref hex) => hex::decode(hex)
            .context("--nonce must be a valid hex string")?,
        None => random_nonce(),
    };

    // Protocol: [cmd:u8][nonce_len:u16 LE][nonce bytes]
    let mut req = vec![CMD_GET_ATTESTATION];
    req.extend_from_slice(&(nonce_bytes.len() as u16).to_le_bytes());
    req.extend_from_slice(&nonce_bytes);

    stream
        .write_all(&req)
        .context("Failed to send attestation request to enclave proxy")?;

    println!("{} Request sent (nonce: {} bytes)", "✔".green(), nonce_bytes.len());

    // ── Read response ────────────────────────────────────────────────────
    // Protocol response: [len:u32 LE][payload bytes]
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

    // The sidecar sends a JSON envelope wrapping the decoded attestation data.
    let envelope: AttestationEnvelope = serde_json::from_slice(&payload)
        .context(
            "Failed to parse attestation envelope. \
             Ensure the sidecar version matches this CLI version.",
        )?;

    // ── Display ──────────────────────────────────────────────────────────
    println!();
    println!("{}", "Attestation Document".bold().yellow());
    println!("  {} Timestamp:  {}", "▶".dimmed(), envelope.timestamp.cyan());
    println!("  {} Public Key: {}", "▶".dimmed(), envelope.public_key.cyan());
    println!();
    println!("{}", "PCR Measurements (from NSM)".bold().yellow());
    println!("  {} PCR0: {}", "▶".dimmed(), envelope.pcr0.cyan());
    println!("  {} PCR1: {}", "▶".dimmed(), envelope.pcr1.cyan());
    println!("  {} PCR2: {}", "▶".dimmed(), envelope.pcr2.cyan());

    // ── Optionally write raw CBOR ────────────────────────────────────────
    if let Some(ref out_path) = args.out {
        let raw = hex::decode(&envelope.raw_cbor_hex)
            .context("Sidecar returned invalid hex for raw_cbor_hex")?;
        std::fs::write(out_path, &raw)
            .with_context(|| format!("Failed to write attestation doc to {}", out_path.display()))?;
        println!();
        println!(
            "{} Raw CBOR attestation doc written to: {}",
            "✔".green(),
            out_path.display()
        );
    }

    println!();
    println!(
        "{} Next: register this enclave on Sui with PCRs + public key above.",
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
