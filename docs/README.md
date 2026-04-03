# Nautilus-Ops

Self-managed TEE orchestrator for AWS Nitro Enclaves on the Sui blockchain. Build enclave images, deploy them to EC2, register attestations on-chain, and verify enclave signatures — all from one CLI.

Supports multiple template types:
- **Rust** — [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) using the `nautilus-enclave` library
- **TypeScript** — [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) (fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts)) using Bun + argonaut
- **Python** — [nautilus-python](https://github.com/Ashwin-3cS/nautilus-python/) using pynacl + stdlib HTTP server
- **Messaging Relayer** — [nautilus-messaging-relayer](https://github.com/Ashwin-3cS/nautilus-messaging-relayer/) adapting Sui Stack Messaging for Nautilus with attestation, signed delivery responses, membership sync, and Walrus archival

## How It Works

Nautilus-Ops is a three-part system:

| Component | What it is | Who uses it |
|-----------|-----------|-------------|
| **nautilus-cli** | CLI binary for building, deploying, and managing enclaves | Developers, from their machine |
| **nautilus-enclave** | Rust library for Ed25519 keygen, signing, and NSM attestation | TEE applications, as a Cargo dependency |
| **contracts/nautilus** | Sui Move smart contract for on-chain attestation and signature verification | dApps, calling `verify_signature()` |

## Architecture

```
Developer Machine                       EC2 Instance (Nitro-enabled)
┌─────────────────────┐                ┌──────────────────────────────────┐
│  nautilus CLI        │                │  Bridge (socat / argonaut)       │
│                      │  SSH / HTTP    │                                  │
│  init                │───────────────>│  ┌────────────────────────────┐  │
│  build               │                │  │   Nitro Enclave (isolated)  │  │
│  init-ci             │                │  │                            │  │
│  deploy-contract     │                │  │   Your TEE App             │  │
│  update-pcrs         │                │  │   (Rust / TS / any lang)   │  │
│  register-enclave    │                │  │   Ed25519 keygen + sign    │  │
│  verify-signature    │                │  │   NSM attestation          │  │
│  attest              │                │  │                            │  │
│  status              │                │  │                            │  │
│  logs                │                │  │                            │  │
│                      │                │  └────────────────────────────┘  │
└─────────────────────┘                └──────────────────────────────────┘
         │  Sui RPC
         v
┌─────────────────────┐
│  Sui Blockchain      │
│                      │
│  EnclaveConfig       │  stores expected PCR values
│  Enclave             │  stores verified Ed25519 public key
│  verify_signature()  │  any dApp can verify enclave signatures
│  verify_signed_data()│  generic raw-bytes verification
└─────────────────────┘
```

## Supported Templates

| Aspect | Rust Template | TS Template | Python Template | Messaging Relayer Template |
|--------|--------------|-------------|-----------------|---------------------------|
| Repo | [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) | [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) (fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts)) | [nautilus-python](https://github.com/Ashwin-3cS/nautilus-python/) | [nautilus-messaging-relayer](https://github.com/Ashwin-3cS/nautilus-messaging-relayer/) |
| Default HTTP port | 4000 | 3000 | 5000 | 4000 |
| Attestation endpoint | `GET /get_attestation` | `GET /attestation` | `GET /attestation` | `GET /get_attestation` |
| Health endpoint | `GET /health` | `GET /health_check` | `GET /health` | `GET /health` |
| Primary action | `POST /sign_name` | `POST /sign` | `POST /sign` | `POST /messages` |
| Logs endpoint | `GET /logs?lines=N` | `GET /logs?lines=N` | `GET /logs?lines=N` | `GET /logs?lines=N` |
| VSOCK bridge | socat (systemd services) | argonaut host binary | socat (systemd services) | socat + outbound host proxies |
| Build | `docker build` + stagex `eif_build` | `make` (docker multi-stage, EIF built inside) | `make` (docker multi-stage, stagex `eif_build`) | `docker build` + stagex `eif_build` |
| Background services | none | none | none | Sui membership sync + Walrus sync |
| Auto-detection | `Cargo.toml` in project root | `argonaut/` dir + `package.json` | `requirements.txt` + `app.py` | `Cargo.toml` + `src/relayer/` |

## Trust Chain

1. **Build** — Docker + `eif_build` produce an Enclave Image File (`.eif`) with deterministic PCR measurements
2. **Deploy** — The Move contract is published to Sui, creating an `EnclaveConfig` and admin `Cap`
3. **Update PCRs** — The expected PCR0/1/2 hashes are written to `EnclaveConfig`
4. **Register** — The CLI fetches a live attestation document from the enclave, submits it on-chain where `sui::nitro_attestation::load_nitro_attestation` verifies the AWS root CA chain and extracts the enclave's Ed25519 public key. If PCRs match, an `Enclave` object is created with the verified key
5. **Verify** — Any dApp calls `verify_signature()` or `verify_signed_data()` to confirm a signature came from the attested enclave

## Repository Structure

```
nautilus-ops/
├── nautilus-cli/                  # CLI binary ("nautilus")
│   └── src/
│       ├── main.rs                # Clap entry point — 11 subcommands
│       ├── init.rs                # nautilus init — scaffold project from template
│       ├── build.rs               # nautilus build — Docker + nitro-cli -> .eif + PCRs
│       ├── init_ci.rs             # nautilus init-ci — generates GitHub Actions workflow
│       ├── status.rs              # nautilus status — health, attestation & on-chain check
│       ├── attest.rs              # nautilus attest — fetch attestation + parse CBOR
│       ├── logs.rs                # nautilus logs — fetch/follow enclave logs
│       ├── aws.rs                 # nautilus verify — EC2 enclave support check
│       ├── sui_chain.rs           # deploy-contract, register-enclave, update-pcrs, verify-signature
│       └── config.rs              # .nautilus.toml persistence, template detection
├── nautilus-enclave/              # Library crate — crypto & attestation primitives
│   └── src/
│       ├── lib.rs                 # Public API: EnclaveKeyPair, get_attestation, verify_signature
│       ├── crypto.rs              # Ed25519 keygen, sign, verify (ed25519-dalek)
│       └── nsm.rs                 # NSM attestation (real with `nsm` feature, mock without)
├── contracts/nautilus/            # Sui Move smart contract
│   ├── Move.toml
│   └── sources/enclave.move       # EnclaveConfig, Enclave, verify_signature, verify_signed_data
├── .nautilus.toml                 # Auto-generated config (package/config/cap object IDs)
├── Cargo.toml                     # Workspace root
└── Cargo.lock
```

## Prerequisites

- **Rust** (stable, 2021 edition)
- **Sui CLI** — [install](https://docs.sui.io/guides/developer/getting-started/sui-install)
- **Docker** — for `nautilus build`
- **AWS EC2** — Nitro-enabled instance (c5.xlarge or similar)

### EC2 Security Group — Inbound Rules

Your EC2 instance must allow inbound traffic on the port your template uses. Add these to your security group:

| Template | Port | Protocol |
|----------|------|----------|
| Rust | 4000 | TCP |
| TypeScript | 3000 | TCP |
| Python | 5000 | TCP |
| Messaging Relayer | 4000 | TCP |

Only open the port for the template you're using. Restrict the source IP to your machine or CI runner if possible.

Confirm Sui CLI is configured:

```bash
sui client active-address   # should print your address
sui client active-env       # should show testnet/mainnet
```

## Installation

```bash
git clone https://github.com/Ashwin-3cS/nautilus-cli.git nautilus-ops
cd nautilus-ops

# Default build (build, init-ci, attest commands)
cargo install --path nautilus-cli

# With on-chain commands (deploy-contract, register-enclave, update-pcrs, verify-signature)
cargo install --path nautilus-cli --features sui

# All features
cargo install --path nautilus-cli --features "sui,aws"
```

### Docker build (no Rust toolchain needed)

If you don't have Rust installed, you can build the CLI using Docker and extract the binary:

```bash
docker build -t nautilus-cli .
docker cp $(docker create nautilus-cli):/usr/local/bin/nautilus /usr/local/bin/nautilus
nautilus --help
```

This uses a multi-stage build with `cargo-chef` for fast cached rebuilds. The extracted binary is Linux x86_64 — Mac/Windows users should use `cargo install` instead.

---

## For TEE App Developers — Using `nautilus-enclave`

If you're building your own TEE application in Rust, add `nautilus-enclave` as a dependency instead of wiring up Ed25519, NSM, and attestation yourself.

### Add the dependency

```toml
# Cargo.toml
[dependencies]
nautilus-enclave = { git = "https://github.com/Ashwin-3cS/nautilus-cli.git" }

[features]
aws = ["nautilus-enclave/nsm"]   # enable real NSM inside enclave
```

### Use it in your app

```rust
use nautilus_enclave::{EnclaveKeyPair, get_attestation};

// Generate Ed25519 keypair (NSM entropy in enclave, OsRng locally)
let kp = EnclaveKeyPair::generate();

// Get attestation document (public key embedded for on-chain verification)
let doc = get_attestation(&kp.public_key_bytes(), b"optional-nonce")?;
// doc.raw_cbor_hex  — the COSE_Sign1 attestation for on-chain submission
// doc.pcr0/pcr1/pcr2 — enclave measurements

// Sign any payload
let sig = kp.sign(&payload_bytes);
```

Three functions. No NSM driver lifecycle, no CBOR parsing, no crypto library selection.

### Mock support

By default (without the `nsm` feature), all NSM calls return deterministic mock data. Your app compiles and runs on your laptop with the same code that runs inside the enclave. No conditional compilation needed in your app code.

### Reference implementations

| Template | Repository | Description |
|----------|-----------|-------------|
| Rust | [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) | Axum sign-server with `/sign_name`, `/get_attestation`, `/health`. Uses `nautilus-enclave` directly |
| TypeScript | [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) | Bun + argonaut framework with `/sign`, `/attestation`, `/health_check`. Fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts) |
| Python | [nautilus-python](https://github.com/Ashwin-3cS/nautilus-python/) | stdlib HTTP server with `/sign`, `/attestation`, `/health`. Uses pynacl for Ed25519, direct NSM ioctl for attestation |

---

## For dApp Developers — CLI Workflow

Once you have a TEE app running inside an enclave, use the CLI to manage the full on-chain lifecycle.

### Full End-to-End Flow

**Step 0: Scaffold a New Project**

```bash
nautilus init --template python my-enclave-app
# Clones the template from GitHub, writes .nautilus.toml, generates CI workflow
# Supported templates: rust, ts, python, messaging-relayer

cd my-enclave-app
```

Or skip this step if you already have a TEE app — `nautilus` auto-detects the template from your project structure.

**Step 1: Build the Enclave Image**

```bash
cd /path/to/your-tee-app

# Rust template
nautilus build -f Containerfile -o out/enclave.eif

# TS template
nautilus build --template ts
# Runs `make`, outputs out/nitro.eif + out/nitro.pcrs
```

**Step 2: Deploy to EC2 via CI**

```bash
nautilus init-ci --cpu-count 2 --memory-mib 4096 -f Containerfile
# Creates .github/workflows/nautilus-deploy.yml
# Template is auto-detected (Cargo.toml + src/relayer -> messaging-relayer, Cargo.toml -> Rust, argonaut/ + package.json -> TS, requirements.txt + app.py -> Python)

# Set GitHub secrets: TEE_EC2_HOST, TEE_EC2_USER, TEE_EC2_SSH_KEY
# Messaging-relayer also needs: RELAYER_SUI_RPC_URL, RELAYER_GROUPS_PACKAGE_ID, RELAYER_WALRUS_PUBLISHER_URL, RELAYER_WALRUS_AGGREGATOR_URL
# Optional for faster Walrus testing: RELAYER_WALRUS_SYNC_INTERVAL_SECS, RELAYER_WALRUS_SYNC_MESSAGE_THRESHOLD
# Push to main -> enclave deploys automatically
```

The generated workflow auto-detects the package manager at runtime — `dnf` on Amazon Linux 2023, `yum` on AL2. Docker, nitro-cli, and all dependencies are installed from a clean instance automatically, no manual setup required.

**Step 3: Deploy the Smart Contract**

```bash
nautilus deploy-contract --network testnet
# Publishes contracts/nautilus/ to Sui
# Saves package_id, config_object_id, cap_object_id to .nautilus.toml
```

**Step 3.5: Copy Config to Template Repo**

The `deploy-contract` command saves object IDs to `.nautilus.toml` in the nautilus-ops directory. Copy this config to your template repo so subsequent commands can read it:

```bash
cp /path/to/nautilus-ops/.nautilus.toml /path/to/your-tee-app/.nautilus.toml
```

Alternatively, pass IDs via flags: `--package-id`, `--config-object-id`, `--cap-object-id`.

**Step 4: Fetch Attestation + Update PCRs**

```bash
# Fetch attestation from running enclave, extract PCRs
nautilus attest --host <EC2_IP> --out pcrs.json
# Parses COSE_Sign1 CBOR document, extracts PCR0/1/2 and public key

# Update expected PCRs on-chain
nautilus update-pcrs --pcr-file pcrs.json
```

**Step 5: Register Enclave On-Chain**

```bash
nautilus register-enclave --host <EC2_IP>
# Fetches attestation, submits on-chain via PTB:
#   1. nitro_attestation::load_nitro_attestation (verifies AWS root CA)
#   2. enclave::register_enclave (creates Enclave object with verified public key)
```

Or combine steps 4+5:

```bash
nautilus register-enclave --host <EC2_IP> --pcr-file pcrs.json
```

**Step 6: Verify a Signature On-Chain**

```bash
# Rust template — signs with IntentMessage<SignedName> + BCS
nautilus verify-signature \
  --host <EC2_IP> \
  --enclave-id <ENCLAVE_OBJECT_ID> \
  --data "Alice"

# TS / Python template — signs blake2b256(raw_data) directly
nautilus verify-signature \
  --template ts \
  --host <EC2_IP> \
  --enclave-id <ENCLAVE_OBJECT_ID> \
  --data "Nautilus"
```

**View Enclave Logs**

Fetch recent logs or follow them in real time:

```bash
# Fetch the last 50 log lines
nautilus logs --host <EC2_IP> -n 50

# Follow logs continuously (like tail -f)
nautilus logs --host <EC2_IP> --follow
```

**Check Status**

At any point, check the health of your entire stack:

```bash
nautilus status --host <EC2_IP>
# ✔ Health:      GET <host>:<port>/health → 200 OK
# ✔ Attestation: GET <host>:<port>/attestation → 200 OK (4503 bytes)
# ✔ On-chain:    config 0x74a8... — PCRs match, enclave 0x5270...
```

### What happens after setup

After steps 1–6, any dApp on Sui can call `verify_signature()` or `verify_signed_data()` in their Move contract to verify that a payload was signed by your attested enclave. The CLI is only needed for setup and management — verification is fully on-chain and permissionless.

---

## CLI Reference

| Command | Description | Requires |
|---------|-------------|----------|
| `nautilus init` | Scaffold a new TEE project from a template (rust/ts/python/messaging-relayer) | git |
| `nautilus build` | Build `.eif` from Dockerfile, extract PCR measurements | Docker |
| `nautilus status` | Check enclave health, attestation, and on-chain PCR status | Enclave running |
| `nautilus logs` | Fetch recent logs or follow live logs from a running enclave | Enclave running |
| `nautilus init-ci` | Generate GitHub Actions deployment workflow | — |
| `nautilus attest` | Fetch attestation from enclave, parse CBOR, extract PCRs | Enclave running |
| `nautilus verify` | Check if an EC2 instance supports Nitro Enclaves | `--features aws` |
| `nautilus deploy-contract` | Publish the Move contract to Sui | `--features sui`, Sui CLI |
| `nautilus update-pcrs` | Set expected PCR values in `EnclaveConfig` | `--features sui`, Sui CLI |
| `nautilus register-enclave` | Register enclave on-chain with attestation | `--features sui`, Sui CLI |
| `nautilus verify-signature` | Verify an enclave signature on-chain | `--features sui`, Sui CLI |

Run `nautilus <command> --help` for full flag details. Use `--template rust|ts|python|messaging-relayer` to override auto-detection (required for `init`, optional for other commands).

## Configuration

The CLI reads and writes `.nautilus.toml` in the current directory:

```toml
[project]
template = "ts"                    # auto-detected or set via --template

[sui]
network = "testnet"
package_id = "0xae07393a..."       # latest deployed package
original_package_id = "0xae07..."  # first-published package (for type args after upgrades)
config_object_id = "0x74a8a2..."
cap_object_id = "0x261f25..."
```

All on-chain commands auto-read these values. You can override with CLI flags or environment variables:

| Flag | Environment Variable |
|------|---------------------|
| `--package-id` | `NAUTILUS_PACKAGE_ID` |
| `--config-object-id` | `NAUTILUS_CONFIG_ID` |
| `--cap-object-id` | `NAUTILUS_CAP_ID` |
| `--host` | `TEE_EC2_HOST` |
| `--enclave-id` | `NAUTILUS_ENCLAVE_ID` |

## Smart Contract

The Move contract at `contracts/nautilus/sources/enclave.move` provides:

### On-Chain Objects

| Object | Description |
|--------|-------------|
| `EnclaveConfig<T>` | Shared object storing expected PCR values and current enclave reference |
| `Enclave<T>` | Shared object storing a verified Ed25519 public key from attestation |
| `Cap<T>` | Admin capability for managing config and registrations |

### Key Functions

**Admin (requires Cap):**

| Function | Visibility | Description |
|----------|-----------|-------------|
| `register_enclave` | `public` | Verify attestation PCRs, extract public key, create `Enclave` object |
| `update_pcrs` | `public` | Update expected PCR0/1/2 in config (invalidates current enclave) |
| `update_name` | `public` | Update the config display name |
| `destroy_old_enclave` | `public` | Clean up old enclave objects after re-registration |

**Verification (permissionless — any dApp can call):**

| Function | Visibility | Description |
|----------|-----------|-------------|
| `verify_signature<T, P>` | `public` | Verify Ed25519 signature over BCS-serialized `IntentMessage<P>`. Returns `bool` |
| `verify_signed_name` | `entry` | Convenience wrapper for `verify_signature` with `SignedName` payload. Aborts if invalid |
| `verify_signed_data<T>` | `entry` | Verify Ed25519 signature over `blake2b256(data)`. For TS template and raw-bytes signing. Aborts if invalid |

### Two Signing Patterns

**Rust template — IntentMessage + BCS:**
```
IntentMessage { intent: u8, timestamp_ms: u64, data: SignedName }
→ BCS serialize → Ed25519 sign
→ on-chain: verify_signed_name() reconstructs and checks
```

**TS / Python template — blake2b256 + raw bytes:**
```
raw_data → blake2b256(data) → Ed25519 sign
→ on-chain: verify_signed_data() hashes and checks
```

### Integrating in Your dApp

```move
module my_app::verified_action;

use nautilus::enclave::{Enclave, verify_signature};

public struct MyPayload has copy, drop {
    action: String,
    value: u64,
}

public fun do_verified_action(
    enclave: &Enclave<nautilus::enclave::ENCLAVE>,
    intent_scope: u8,
    timestamp_ms: u64,
    action: String,
    value: u64,
    signature: &vector<u8>,
) {
    let payload = MyPayload { action, value };
    let valid = verify_signature(enclave, intent_scope, timestamp_ms, payload, signature);
    assert!(valid, 0);
    // ... proceed with trusted action
}
```

## Feature Flags

| Crate | Feature | Default | Purpose |
|-------|---------|---------|---------|
| `nautilus-cli` | `sui` | off | Enables on-chain commands. Adds `reqwest`, `ciborium` dependencies |
| `nautilus-cli` | `aws` | off | Enables EC2 enclave support check. Adds `aws-sdk-ec2` |
| `nautilus-enclave` | `nsm` | off | Enables real NSM device calls. Only works inside a Nitro Enclave |

## Running Tests

```bash
# All tests across all crates (uses mocks, no enclave needed)
cargo test

# Individual crates
cargo test -p nautilus-enclave                    # 7 tests — crypto + attestation
cargo test -p nautilus-cli                        # 28 tests — CLI, config, build, init-ci
cargo test -p nautilus-cli --features sui         # includes on-chain config tests
```

## Related Repositories

| Repository | Description |
|-----------|-------------|
| [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) | Rust TEE template — Axum sign-server powered by `nautilus-enclave`. Endpoints: `/sign_name`, `/get_attestation`, `/health`, `/logs` |
| [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) | TypeScript TEE template — Bun + argonaut framework. Fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts). Endpoints: `/sign`, `/attestation`, `/health_check` |
| [nautilus-python](https://github.com/Ashwin-3cS/nautilus-python/) | Python TEE template — stdlib HTTP server with pynacl Ed25519 and direct NSM ioctl. Endpoints: `/sign`, `/attestation`, `/health` |
| [nautilus-messaging-relayer](https://github.com/Ashwin-3cS/nautilus-messaging-relayer/) | Messaging relayer TEE template — Axum relayer adapted for Nautilus. Endpoints: `/messages`, `/get_attestation`, `/health`, `/health_check`, `/logs` |

## Security

This CLI and its associated smart contracts have **not been security audited**. Use at your own risk. It is intended for development, testing, and educational purposes. If you plan to use it in production with real assets, you should conduct a thorough security review of the CLI, the Move contract, and your enclave application before deployment.

## License

MIT
