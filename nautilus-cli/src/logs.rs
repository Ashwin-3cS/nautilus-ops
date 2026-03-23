use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::config::{self, Template};

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// EC2 instance public hostname or IP.
    #[arg(long, env = "TEE_EC2_HOST")]
    pub host: String,

    /// Override the default TCP port for the template.
    #[arg(long)]
    pub port: Option<u16>,

    /// Number of recent log lines to fetch (default: 100, max: 1000).
    #[arg(long, short = 'n', default_value = "100")]
    pub lines: usize,

    /// Poll for new logs continuously (every 2 seconds).
    #[arg(long, short = 'f')]
    pub follow: bool,
}

pub async fn run(args: LogsArgs, cli_template: Option<Template>) -> Result<()> {
    let cfg = config::NautilusConfig::load(None).unwrap_or_default();
    let template = config::resolve_template(cli_template, &cfg)?;
    let port = args.port.unwrap_or(template.default_http_port());
    let path = template.logs_path();
    let n = args.lines.min(1000);

    if args.follow {
        follow_logs(&args.host, port, path, n)?;
    } else {
        let lines = fetch_logs(&args.host, port, path, n)?;
        for line in &lines {
            println!("{}", line);
        }
        if lines.is_empty() {
            println!("{}", "No log lines available.".dimmed());
        }
    }

    Ok(())
}

/// Fetch log lines from the enclave's /logs endpoint.
fn fetch_logs(host: &str, port: u16, path: &str, n: usize) -> Result<Vec<String>> {
    let url_path = format!("{}?lines={}", path, n);
    let addr = format!("{host}:{port}");

    let mut stream = TcpStream::connect(&addr)
        .with_context(|| format!("Cannot reach enclave at {addr}"))?;

    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let req = format!(
        "GET {url_path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes())?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response_str = String::from_utf8_lossy(&response);

    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .context("Invalid HTTP response")?;

    let json: serde_json::Value =
        serde_json::from_str(body).context("Failed to parse logs JSON")?;

    let lines = json["lines"]
        .as_array()
        .context("Response missing 'lines' array")?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    Ok(lines)
}

/// Continuously poll for new logs every 2 seconds.
fn follow_logs(host: &str, port: u16, path: &str, initial_n: usize) -> Result<()> {
    println!(
        "{} Following logs from {}:{} (Ctrl+C to stop)",
        "→".cyan(),
        host,
        port
    );

    let mut last_count = 0;

    // First fetch: show initial_n lines
    match fetch_logs(host, port, path, initial_n) {
        Ok(lines) => {
            for line in &lines {
                println!("{}", line);
            }
            last_count = lines.len();
        }
        Err(e) => {
            eprintln!("{} {}", "✗".red(), e);
        }
    }

    loop {
        std::thread::sleep(Duration::from_secs(2));

        match fetch_logs(host, port, path, 1000) {
            Ok(lines) => {
                // Print only new lines (lines beyond what we last saw)
                if lines.len() > last_count {
                    for line in &lines[last_count..] {
                        println!("{}", line);
                    }
                }
                last_count = lines.len();
            }
            Err(_) => {
                // Silently retry on connection errors in follow mode
            }
        }
    }
}
