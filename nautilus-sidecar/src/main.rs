// nautilus-sidecar/src/main.rs
//
// Entry point for the Nautilus enclave sidecar.
// Runs *inside* the AWS Nitro Enclave — no network, only VSOCK to the parent host.
//
// Boot sequence:
//   1. Generate an Ed25519 keypair (fresh on every enclave start).
//   2. Start a VSOCK listener on port 5000.
//   3. Handle incoming commands from the EC2 host:
//      - GET_ATTESTATION (0x01) → ask NSM to include our pubkey, return JSON envelope.
//      - SIGN (0x02)            → sign an arbitrary payload with our private key.

mod crypto;
mod nsm;
mod vsock;

use anyhow::Result;
use colored::Colorize;
use crypto::EnclaveKeyPair;

fn main() -> Result<()> {
    eprintln!("{}", "Nautilus Enclave Sidecar".bold().cyan());
    eprintln!("{}", "─".repeat(40).dimmed());

    // ── Step 1: Generate Ed25519 keypair ────────────────────────────────
    eprintln!("{} Generating Ed25519 keypair…", "→".cyan());
    let keypair = EnclaveKeyPair::generate();
    eprintln!(
        "{} Public key: {}",
        "✔".green(),
        hex::encode(keypair.public_key_bytes()).cyan()
    );

    // ── Step 2: Start VSOCK listener ─────────────────────────────────────
    eprintln!("{} Starting VSOCK listener on port 5000…", "→".cyan());
    vsock::run_server(keypair)?;

    Ok(())
}
