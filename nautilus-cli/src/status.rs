use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::config::{self, Template};

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// EC2 instance public hostname or IP.
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// Override the default TCP port for the template.
    #[arg(long)]
    pub port: Option<u16>,
}

pub async fn run(args: StatusArgs, cli_template: Option<Template>) -> Result<()> {
    let cfg = config::NautilusConfig::load(None).unwrap_or_default();
    let template = config::resolve_template(cli_template, &cfg)?;
    let port = args.port.unwrap_or(template.default_http_port());

    println!("{}", "Nautilus Status".bold().cyan());
    println!(
        "{} Template: {}  Host: {}:{}",
        "→".cyan(),
        format!("{template}").bold(),
        args.host.cyan(),
        port.to_string().cyan()
    );
    println!("{}", "─".repeat(50).dimmed());

    // 1. Health check
    let health_ok = check_health(&args.host, port, template);

    // 2. Attestation check
    let attest_result = check_attestation(&args.host, port, template);

    // 3. On-chain check (only if config has IDs)
    let has_onchain = cfg.sui.package_id.is_some() && cfg.sui.config_object_id.is_some();
    if has_onchain {
        check_onchain(&cfg, &attest_result, cfg.sui.network.as_deref());
    } else {
        println!(
            "  {} On-chain:    {} (no package_id/config_object_id in .nautilus.toml)",
            "–".dimmed(),
            "skipped".dimmed()
        );
    }

    // Summary
    println!("{}", "─".repeat(50).dimmed());
    if health_ok && attest_result.is_some() {
        println!("{} Enclave is healthy and responding.", "✔".green().bold());
    } else if health_ok {
        println!(
            "{} Enclave is reachable but attestation failed.",
            "⚠".yellow().bold()
        );
    } else {
        println!(
            "{} Enclave is not reachable at {}:{}.",
            "✗".red().bold(),
            args.host,
            port
        );
    }

    Ok(())
}

/// Hit the health endpoint. Returns true if 200-level response.
fn check_health(host: &str, port: u16, template: Template) -> bool {
    let path = template.health_path();
    let addr = format!("{host}:{port}");

    match http_get(host, port, path) {
        Ok((status, body)) if status >= 200 && status < 300 => {
            println!(
                "  {} Health:      {} {} → {}",
                "✔".green(),
                "GET".dimmed(),
                format!("{addr}{path}").cyan(),
                format!("{status} OK").green()
            );
            // Show response body if it's short JSON
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(obj) = json.as_object() {
                    for (k, v) in obj {
                        println!("                   {} {}: {}", "·".dimmed(), k.dimmed(), v.to_string().dimmed());
                    }
                }
            }
            true
        }
        Ok((status, _)) => {
            println!(
                "  {} Health:      {} {} → {}",
                "✗".red(),
                "GET".dimmed(),
                format!("{addr}{path}").cyan(),
                format!("{status}").red()
            );
            false
        }
        Err(e) => {
            println!(
                "  {} Health:      {} ({})",
                "✗".red(),
                "unreachable".red(),
                short_error(&e)
            );
            false
        }
    }
}

/// Hit the attestation endpoint. Returns Some((pcrs, pubkey)) on success.
fn check_attestation(
    host: &str,
    port: u16,
    template: Template,
) -> Option<AttestInfo> {
    let path = template.attestation_path();
    let addr = format!("{host}:{port}");

    match http_get(host, port, path) {
        Ok((status, body)) if status >= 200 && status < 300 => {
            // Try to parse the attestation
            match parse_attest_response(&body, template) {
                Ok(info) => {
                    println!(
                        "  {} Attestation: {} {} → {} ({} bytes)",
                        "✔".green(),
                        "GET".dimmed(),
                        format!("{addr}{path}").cyan(),
                        format!("{status} OK").green(),
                        info.doc_len
                    );
                    println!(
                        "                   {} public_key: {}...",
                        "·".dimmed(),
                        &info.public_key[..info.public_key.len().min(16)].dimmed()
                    );
                    for (i, pcr) in info.pcrs.iter().enumerate() {
                        println!(
                            "                   {} PCR{}: {}...",
                            "·".dimmed(),
                            i,
                            &pcr[..pcr.len().min(16)].dimmed()
                        );
                    }
                    Some(info)
                }
                Err(e) => {
                    println!(
                        "  {} Attestation: {} {} → {} but parse failed ({})",
                        "⚠".yellow(),
                        "GET".dimmed(),
                        format!("{addr}{path}").cyan(),
                        format!("{status}").green(),
                        short_error(&e)
                    );
                    None
                }
            }
        }
        Ok((status, _)) => {
            println!(
                "  {} Attestation: {} {} → {}",
                "✗".red(),
                "GET".dimmed(),
                format!("{addr}{path}").cyan(),
                format!("{status}").red()
            );
            None
        }
        Err(e) => {
            println!(
                "  {} Attestation: {} ({})",
                "✗".red(),
                "unreachable".red(),
                short_error(&e)
            );
            None
        }
    }
}

/// Check on-chain config PCRs against live attestation PCRs.
fn check_onchain(cfg: &config::NautilusConfig, attest_info: &Option<AttestInfo>, network: Option<&str>) {
    let config_id = cfg.sui.config_object_id.as_deref().unwrap();
    let package_id = cfg.sui.package_id.as_deref().unwrap();

    // Query on-chain object
    match query_onchain_config(config_id, network) {
        Ok(onchain) => {
            println!(
                "  {} On-chain:    config {} ({})",
                "✔".green(),
                &config_id[..config_id.len().min(12)].cyan(),
                format!("pkg {}", &package_id[..package_id.len().min(12)]).dimmed()
            );

            // Compare PCRs if we have attestation info
            if let Some(info) = attest_info {
                if info.pcrs.len() >= 3
                    && onchain.pcrs.len() >= 3
                    && info.pcrs[0] == onchain.pcrs[0]
                    && info.pcrs[1] == onchain.pcrs[1]
                    && info.pcrs[2] == onchain.pcrs[2]
                {
                    println!(
                        "                   {} PCRs match on-chain config",
                        "✔".green()
                    );
                } else {
                    println!(
                        "                   {} PCR mismatch — run {} then {}",
                        "⚠".yellow(),
                        "nautilus update-pcrs".cyan(),
                        "nautilus register-enclave".cyan()
                    );
                }
            }

            // Show enclave reference if present
            if let Some(ref enc_id) = onchain.enclave_id {
                println!(
                    "                   {} enclave: {}",
                    "·".dimmed(),
                    &enc_id[..enc_id.len().min(16)].dimmed()
                );
            } else {
                println!(
                    "                   {} no enclave registered — run {}",
                    "–".dimmed(),
                    "nautilus register-enclave".cyan()
                );
            }
        }
        Err(e) => {
            println!(
                "  {} On-chain:    failed to query {} ({})",
                "✗".red(),
                &config_id[..config_id.len().min(12)],
                short_error(&e)
            );
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

struct AttestInfo {
    public_key: String,
    pcrs: Vec<String>,
    doc_len: usize,
}

struct OnchainConfig {
    pcrs: Vec<String>,
    enclave_id: Option<String>,
}

/// Simple HTTP GET via raw TCP. Returns (status_code, body).
fn http_get(host: &str, port: u16, path: &str) -> Result<(u16, String)> {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("Connection refused at {addr}"))?;

    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes())?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response_str = String::from_utf8_lossy(&response);

    // Parse status code from first line: "HTTP/1.1 200 OK"
    let status_line = response_str
        .lines()
        .next()
        .context("Empty HTTP response")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .context("No status code in response")?
        .parse()
        .context("Invalid status code")?;

    // Extract body after \r\n\r\n
    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .to_string();

    Ok((status, body))
}

/// Parse attestation response JSON + CBOR to extract PCRs and public key.
fn parse_attest_response(body: &str, _template: Template) -> Result<AttestInfo> {
    let json: serde_json::Value = serde_json::from_str(body)
        .context("Invalid JSON in attestation response")?;

    let attestation_hex = json["attestation"]
        .as_str()
        .context("Missing 'attestation' field")?;

    let attestation_bytes = hex::decode(attestation_hex)
        .context("Attestation is not valid hex")?;

    let doc_len = attestation_bytes.len();

    // Parse COSE_Sign1 → payload → CBOR map
    let cose: ciborium::value::Value =
        ciborium::de::from_reader(&attestation_bytes[..])
            .context("Failed to decode COSE_Sign1 CBOR")?;

    let payload_bytes = extract_cose_payload(&cose)?;
    let payload: ciborium::value::Value =
        ciborium::de::from_reader(&payload_bytes[..])
            .context("Failed to decode attestation payload CBOR")?;

    let public_key = extract_cbor_bytes(&payload, "public_key")
        .map(hex::encode)
        .unwrap_or_default();

    let pcrs = extract_pcrs(&payload)?;

    Ok(AttestInfo {
        public_key,
        pcrs,
        doc_len,
    })
}

/// Extract payload bytes from COSE_Sign1 structure.
fn extract_cose_payload(cose: &ciborium::value::Value) -> Result<Vec<u8>> {
    // COSE_Sign1 can be Tag(18, [...]) or just [...]
    let arr = match cose {
        ciborium::value::Value::Tag(18, inner) => {
            inner.as_array().context("COSE_Sign1 tag content is not an array")?
        }
        ciborium::value::Value::Array(arr) => arr,
        _ => anyhow::bail!("Expected COSE_Sign1 array"),
    };

    // [protected, unprotected, payload, signature]
    arr.get(2)
        .and_then(|v| v.as_bytes())
        .map(|b| b.to_vec())
        .context("COSE_Sign1 payload is not bytes")
}

/// Extract a bytes field from a CBOR map by string key.
fn extract_cbor_bytes(
    value: &ciborium::value::Value,
    key: &str,
) -> Option<Vec<u8>> {
    let map = value.as_map()?;
    for (k, v) in map {
        if let Some(k_str) = k.as_text() {
            if k_str == key {
                return v.as_bytes().map(|b| b.to_vec());
            }
        }
    }
    None
}

/// Extract PCR0, PCR1, PCR2 from the attestation payload.
fn extract_pcrs(payload: &ciborium::value::Value) -> Result<Vec<String>> {
    let map = payload.as_map().context("Payload is not a CBOR map")?;

    // Find "pcrs" key
    let pcrs_value = map
        .iter()
        .find(|(k, _)| k.as_text() == Some("pcrs"))
        .map(|(_, v)| v)
        .context("No 'pcrs' field in attestation payload")?;

    let pcrs_map = pcrs_value.as_map().context("pcrs is not a CBOR map")?;

    let mut result = Vec::new();
    for idx in 0..3 {
        let pcr = pcrs_map
            .iter()
            .find(|(k, _)| k.as_integer() == Some(ciborium::value::Integer::from(idx)))
            .map(|(_, v)| v)
            .and_then(|v| v.as_bytes())
            .map(hex::encode)
            .unwrap_or_default();
        result.push(pcr);
    }

    Ok(result)
}

/// Query on-chain EnclaveConfig object via Sui JSON-RPC.
fn query_onchain_config(config_id: &str, network: Option<&str>) -> Result<OnchainConfig> {
    let rpc_url = match network.unwrap_or("testnet") {
        "mainnet" => "https://fullnode.mainnet.sui.io:443",
        "devnet" => "https://fullnode.devnet.sui.io:443",
        _ => "https://fullnode.testnet.sui.io:443",
    };

    let req_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sui_getObject",
        "params": [config_id, {"showContent": true}]
    });

    let output = std::process::Command::new("curl")
        .args([
            "-s", "-X", "POST", rpc_url,
            "-H", "Content-Type: application/json",
            "-d", &req_body.to_string(),
        ])
        .output()
        .context("Failed to run curl for Sui JSON-RPC")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Sui RPC request failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .context("Failed to parse Sui RPC response")?;

    if let Some(err) = json.get("error") {
        anyhow::bail!("Sui RPC error: {}", err);
    }

    let fields = &json["result"]["data"]["content"]["fields"];

    // PCRs: stored as byte arrays in fields.pcrs.fields.pos0/pos1/pos2
    let pcr_fields = &fields["pcrs"]["fields"];
    let pcr0 = bytes_array_to_hex(&pcr_fields["pos0"]);
    let pcr1 = bytes_array_to_hex(&pcr_fields["pos1"]);
    let pcr2 = bytes_array_to_hex(&pcr_fields["pos2"]);

    // Enclave ID: fields.current_enclave_id (hex string or null)
    let enclave_id = fields["current_enclave_id"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != "0x0000000000000000000000000000000000000000000000000000000000000000")
        .map(|s| s.to_string());

    Ok(OnchainConfig {
        pcrs: vec![pcr0, pcr1, pcr2],
        enclave_id,
    })
}

/// Convert a JSON array of u8 values to a hex string.
fn bytes_array_to_hex(value: &serde_json::Value) -> String {
    match value.as_array() {
        Some(arr) => {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            hex::encode(bytes)
        }
        None => String::new(),
    }
}

/// Shorten an error message for inline display.
fn short_error(e: &anyhow::Error) -> String {
    let msg = e.to_string();
    if msg.len() > 60 {
        format!("{}...", &msg[..57])
    } else {
        msg
    }
}
