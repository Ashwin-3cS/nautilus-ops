// nautilus-sidecar/src/vsock.rs
//
// VSOCK server that listens for commands from the EC2 host.
// Runs on AF_VSOCK port 5000 inside the Nitro Enclave.
//
// Protocol:
//   Request:  [cmd:u8][payload_len:u16 LE][payload bytes]
//   Response: [len:u32 LE][JSON payload bytes]
//
// Commands:
//   0x01 GET_ATTESTATION — payload is the nonce; returns AttestationEnvelope JSON
//   0x02 SIGN            — payload is the message to sign; returns SignResponse JSON

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::io::{Read, Write};

use nautilus_enclave::EnclaveKeyPair;
use nautilus_enclave as nsm;

/// VSOCK port the sidecar listens on.
const VSOCK_PORT: u32 = 5000;

/// Any CID — accept connections from the parent EC2 instance.
const VMADDR_CID_ANY: u32 = 0xFFFFFFFF;

/// Command byte: request an NSM attestation document.
const CMD_GET_ATTESTATION: u8 = 0x01;

/// Command byte: sign an arbitrary payload with the enclave's Ed25519 key.
const CMD_SIGN: u8 = 0x02;

/// JSON response for a SIGN command.
#[derive(Debug, Serialize)]
struct SignResponse {
    /// Hex-encoded Ed25519 signature (64 bytes = 128 hex chars).
    signature: String,
    /// Hex-encoded public key that produced this signature.
    public_key: String,
}

/// Start the VSOCK server loop. This blocks forever, handling one client at a time.
pub fn run_server(keypair: EnclaveKeyPair) -> Result<()> {
    let listener = vsock::VsockListener::bind_with_cid_port(VMADDR_CID_ANY, VSOCK_PORT)
        .context("Failed to bind VSOCK listener on port 5000. Is AF_VSOCK available?")?;

    eprintln!(
        "{} VSOCK server listening on port {}",
        "✔".green(),
        VSOCK_PORT
    );

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                eprintln!("{} Client connected", "→".cyan());
                if let Err(e) = handle_client(&mut stream, &keypair) {
                    eprintln!("{} Error handling client: {:#}", "✗".red(), e);
                }
            }
            Err(e) => {
                eprintln!("{} Accept error: {:#}", "✗".red(), e);
            }
        }
    }

    Ok(())
}

/// Handle a single client connection: read one command, send one response, then close.
fn handle_client(stream: &mut vsock::VsockStream, keypair: &EnclaveKeyPair) -> Result<()> {
    // ── Read command header ───────────────────────────────────────────────
    let mut cmd_buf = [0u8; 1];
    stream
        .read_exact(&mut cmd_buf)
        .context("Failed to read command byte")?;
    let cmd = cmd_buf[0];

    let mut len_buf = [0u8; 2];
    stream
        .read_exact(&mut len_buf)
        .context("Failed to read payload length")?;
    let payload_len = u16::from_le_bytes(len_buf) as usize;

    // Sanity check: reject payloads larger than 8 KiB
    if payload_len > 8192 {
        anyhow::bail!("Payload too large: {} bytes (max 8192)", payload_len);
    }

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream
            .read_exact(&mut payload)
            .context("Failed to read payload")?;
    }

    // ── Dispatch command ──────────────────────────────────────────────────
    let response_json = match cmd {
        CMD_GET_ATTESTATION => {
            eprintln!("  {} CMD: GET_ATTESTATION (nonce: {} bytes)", "▶".dimmed(), payload.len());
            handle_attestation(keypair, &payload)?
        }
        CMD_SIGN => {
            eprintln!("  {} CMD: SIGN (payload: {} bytes)", "▶".dimmed(), payload.len());
            handle_sign(keypair, &payload)?
        }
        _ => {
            anyhow::bail!("Unknown command byte: 0x{:02X}", cmd);
        }
    };

    // ── Send response ─────────────────────────────────────────────────────
    // Protocol: [len:u32 LE][JSON bytes]
    let response_bytes = response_json.as_bytes();
    let len = (response_bytes.len() as u32).to_le_bytes();
    stream.write_all(&len).context("Failed to write response length")?;
    stream
        .write_all(response_bytes)
        .context("Failed to write response payload")?;
    stream.flush().context("Failed to flush response")?;

    eprintln!("  {} Response sent ({} bytes)", "✔".green(), response_bytes.len());
    Ok(())
}

/// Handle GET_ATTESTATION: call NSM with the enclave's public key and the client's nonce.
fn handle_attestation(keypair: &EnclaveKeyPair, nonce: &[u8]) -> Result<String> {
    let pub_bytes = keypair.public_key_bytes();
    let doc = nsm::get_attestation(&pub_bytes, nonce)
        .context("NSM attestation request failed")?;

    serde_json::to_string(&doc).context("Failed to serialize attestation envelope")
}

/// Handle SIGN: sign the payload with the enclave's Ed25519 private key.
fn handle_sign(keypair: &EnclaveKeyPair, payload: &[u8]) -> Result<String> {
    let signature = keypair.sign(payload);
    let response = SignResponse {
        signature: hex::encode(signature.to_bytes()),
        public_key: hex::encode(keypair.public_key_bytes()),
    };

    serde_json::to_string(&response).context("Failed to serialize sign response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_attestation_returns_valid_json() {
        let kp = EnclaveKeyPair::generate();
        let nonce = b"test-nonce-12345";
        let json = handle_attestation(&kp, nonce).unwrap();

        let doc: nsm::AttestationDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(doc.public_key, hex::encode(kp.public_key_bytes()));
        assert!(!doc.pcr0.is_empty());
    }

    #[test]
    fn test_handle_sign_returns_valid_signature() {
        let kp = EnclaveKeyPair::generate();
        let payload = b"hello nautilus";
        let json = handle_sign(&kp, payload).unwrap();

        let resp: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sig_hex = resp["signature"].as_str().unwrap();
        let pub_hex = resp["public_key"].as_str().unwrap();

        // Verify the signature is valid
        let sig_bytes: [u8; 64] = hex::decode(sig_hex)
            .unwrap()
            .try_into()
            .unwrap();
        let pub_bytes: [u8; 32] = hex::decode(pub_hex)
            .unwrap()
            .try_into()
            .unwrap();

        assert!(nautilus_enclave::verify_signature(&pub_bytes, payload, &sig_bytes).is_ok());
    }

    #[test]
    fn test_sign_response_serialization() {
        let resp = SignResponse {
            signature: "aa".repeat(64),
            public_key: "bb".repeat(32),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("signature"));
        assert!(json.contains("public_key"));
    }
}
