# Nautilus-Ops

Self-managed TEE orchestrator for AWS Nitro Enclaves on the Sui blockchain. Build enclave images, deploy them to EC2, register attestations on-chain, and verify enclave signatures — all from one CLI.

> **Looking for the TEE application that runs inside the enclave?** See [nautilus-tee-app](https://github.com/Ashwin-3cS/nautilus-tee-app/) — a reference Axum sign-server that uses the `nautilus-enclave` library for crypto and attestation. This CLI orchestrates that application.

## How It Works

Nautilus-Ops is a three-part system:

| Component | What it is | Who uses it |
|-----------|-----------|-------------|
| **nautilus-cli** | CLI binary for building, deploying, and managing enclaves | Developers, from their machine |
| **nautilus-enclave** | Rust library for Ed25519 keygen, signing, and NSM attestation | TEE applications, as a Cargo dependency |
| **contracts/nautilus** | Sui Move smart contract for on-chain attestation and signature verification | dApps, calling `verify_signature()` |

The `nautilus-sidecar` is an optional pre-built VSOCK binary that wraps the library — useful if your enclave app isn't written in Rust.

## Architecture

```
Developer Machine                       EC2 Instance (Nitro-enabled)
┌─────────────────────┐                ┌──────────────────────────────────┐
│  nautilus CLI        │                │  socat bridge (TCP:4000 <-> VSOCK)│
│                      │  SSH / HTTP    │                                  │
│  build               │───────────────>│  ┌────────────────────────────┐  │
│  init-ci             │                │  │   Nitro Enclave (isolated)  │  │
│  deploy-contract     │                │  │                            │  │
│  update-pcrs         │                │  │   Your TEE App             │  │
│  register-enclave    │                │  │   └── nautilus-enclave     │  │
│  verify-signature    │                │  │       (keygen, sign, attest)│  │
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
└─────────────────────┘
```

## Trust Chain

1. **Build** — Docker + `eif_build` produce an Enclave Image File (`.eif`) with deterministic PCR measurements
2. **Deploy** — The Move contract is published to Sui, creating an `EnclaveConfig` and admin `Cap`
3. **Update PCRs** — The expected PCR0/1/2 hashes are written to `EnclaveConfig`
4. **Register** — The CLI fetches a live attestation document from the enclave, submits it on-chain where `sui::nitro_attestation::load_nitro_attestation` verifies the AWS root CA chain and extracts the enclave's Ed25519 public key. If PCRs match, an `Enclave` object is created with the verified key
5. **Verify** — Any dApp calls `verify_signature(enclave, intent, timestamp, payload, sig)` to confirm a signature came from the attested enclave

## Repository Structure

```
nautilus-ops/
├── nautilus-cli/                  # CLI binary ("nautilus")
│   └── src/
│       ├── main.rs                # Clap entry point — 8 subcommands
│       ├── build.rs               # nautilus build — Docker + nitro-cli -> .eif + PCRs
│       ├── init_ci.rs             # nautilus init-ci — generates GitHub Actions workflow
│       ├── attest.rs              # nautilus attest — fetch attestation via TCP->VSOCK
│       ├── aws.rs                 # nautilus verify — EC2 enclave support check
│       ├── sui_chain.rs           # deploy-contract, register-enclave, update-pcrs, verify-signature
│       └── config.rs              # .nautilus.toml persistence
├── nautilus-enclave/              # Library crate — crypto & attestation primitives
│   └── src/
│       ├── lib.rs                 # Public API: EnclaveKeyPair, get_attestation, verify_signature
│       ├── crypto.rs              # Ed25519 keygen, sign, verify (ed25519-dalek)
│       └── nsm.rs                 # NSM attestation (real with `nsm` feature, mock without)
├── nautilus-sidecar/              # Optional VSOCK binary (for non-Rust enclave apps)
│   └── src/
│       ├── main.rs                # Boot: keygen -> VSOCK server on port 5000
│       └── vsock.rs               # Binary protocol: GET_ATTESTATION (0x01), SIGN (0x02)
├── contracts/nautilus/            # Sui Move smart contract
│   ├── Move.toml
│   └── sources/enclave.move       # EnclaveConfig, Enclave, verify_signature, verify_signed_name
├── .nautilus.toml                 # Auto-generated config (package/config/cap object IDs)
├── Cargo.toml                     # Workspace root
└── Cargo.lock
```

## Prerequisites

- **Rust** (stable, 2021 edition)
- **Sui CLI** — [install](https://docs.sui.io/guides/developer/getting-started/sui-install)
- **Docker** — for `nautilus build`
- **AWS EC2** — Nitro-enabled instance (c5.xlarge or similar)

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

### What it replaces

Without `nautilus-enclave`, a TEE app has to do all of this:

```rust
// Before — ~60 lines of boilerplate per app
use fastcrypto::ed25519::Ed25519KeyPair;
use aws_nitro_enclaves_nsm_api::{driver, api::Request};

let fd = driver::nsm_init();
let req = Request::Attestation { user_data: None, nonce: None,
    public_key: Some(ByteBuf::from(pk.as_bytes().to_vec())) };
let resp = driver::nsm_process_request(fd, req);
match resp { Response::Attestation { document } => { /* hex encode, extract PCRs... */ } }
driver::nsm_exit(fd);
// + figure out keygen, entropy, mock/real branching, feature gating...
```

With `nautilus-enclave`:

```rust
// After — 3 lines
let kp = EnclaveKeyPair::generate();
let doc = get_attestation(&kp.public_key_bytes(), &[])?;
let sig = kp.sign(&payload);
```

### Mock support

By default (without the `nsm` feature), all NSM calls return deterministic mock data. Your app compiles and runs on your laptop with the same code that runs inside the enclave. No conditional compilation needed in your app code.

### Reference implementation

See [nautilus-tee-app](https://github.com/Ashwin-3cS/nautilus-tee-app/) for a complete working example — an Axum sign-server with `/sign_name`, `/get_attestation`, and `/health` endpoints, all powered by `nautilus-enclave`.

---

## For dApp Developers — CLI Workflow

Once you have a TEE app running inside an enclave, use the CLI to manage the full on-chain lifecycle.

### Full End-to-End Flow

**Step 1: Build the Enclave Image**

```bash
cd /path/to/your-tee-app

nautilus build -f Containerfile -o out/enclave.eif
# Outputs: out/enclave.eif + out/enclave.eif.pcrs.json
```

**Step 2: Deploy to EC2 via CI**

```bash
nautilus init-ci --cpu-count 2 --memory-mib 4096 -f Containerfile
# Creates .github/workflows/nautilus-deploy.yml

# Set GitHub secrets: TEE_EC2_HOST, TEE_EC2_USER, TEE_EC2_SSH_KEY
# Push to main -> enclave deploys automatically
```

**Step 3: Deploy the Smart Contract**

```bash
nautilus deploy-contract --network testnet
# Publishes contracts/nautilus/ to Sui
# Saves package_id, config_object_id, cap_object_id to .nautilus.toml
```

**Step 4: Update PCRs + Register Enclave**

One command to set PCRs and register:

```bash
nautilus register-enclave --host <EC2_IP> --pcr-file out/enclave.eif.pcrs.json
```

Or do them separately:

```bash
nautilus update-pcrs --pcr-file out/enclave.eif.pcrs.json
nautilus register-enclave --host <EC2_IP>
```

**Step 5: Verify a Signature On-Chain**

```bash
nautilus verify-signature \
  --host <EC2_IP> \
  --enclave-id <ENCLAVE_OBJECT_ID> \
  --name "Alice"
# Calls /sign_name on the enclave
# Submits the signature to on-chain verify_signed_name()
# Transaction succeeds only if the signature is valid
```

### What happens after setup

After steps 1–4, any dApp on Sui can call `verify_signature()` in their Move contract to verify that a payload was signed by your attested enclave. The CLI is only needed for setup and management — verification is fully on-chain and permissionless.

## CLI Reference

| Command | Description | Requires |
|---------|-------------|----------|
| `nautilus build` | Build `.eif` from Dockerfile, extract PCR measurements | Docker |
| `nautilus init-ci` | Generate GitHub Actions deployment workflow | — |
| `nautilus attest` | Fetch raw attestation document from a running enclave | Enclave running |
| `nautilus verify` | Check if an EC2 instance supports Nitro Enclaves | `--features aws` |
| `nautilus deploy-contract` | Publish the Move contract to Sui | `--features sui`, Sui CLI |
| `nautilus update-pcrs` | Set expected PCR values in `EnclaveConfig` | `--features sui`, Sui CLI |
| `nautilus register-enclave` | Register enclave on-chain with attestation | `--features sui`, Sui CLI |
| `nautilus verify-signature` | Verify an enclave signature on-chain | `--features sui`, Sui CLI |

Run `nautilus <command> --help` for full flag details.

## Configuration

The CLI reads and writes `.nautilus.toml` in the current directory. After `deploy-contract`, it looks like:

```toml
[sui]
network = "testnet"
package_id = "0x441c8612..."
config_object_id = "0x61d547cb..."
cap_object_id = "0xfed6e7d8..."
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
| `verify_signature<T, P>` | `public` | Verify an Ed25519 signature against an enclave's stored public key. Reconstructs `IntentMessage<P>` from args, BCS-serializes, and checks the signature. Returns `bool` |
| `verify_signed_name` | `entry` | Convenience wrapper that calls `verify_signature` with a `SignedName` payload. Aborts if invalid |

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

### BCS Compatibility

The on-chain `IntentMessage<T>` and the tee-app's Rust `IntentMessage<T>` produce identical BCS bytes because they share the same field order:

```
Move:  IntentMessage { intent: u8, timestamp_ms: u64, payload: T }
Rust:  IntentMessage { intent: u8, timestamp_ms: u64, data: T }
                                                      ^^^^ field name doesn't matter for BCS
```

BCS serializes by field order, not field name.

## Sidecar (Optional)

The `nautilus-sidecar` is a pre-built VSOCK binary for cases where your enclave app isn't in Rust. It wraps `nautilus-enclave` behind a binary protocol on VSOCK port 5000:

```
Request:  [cmd: u8] [payload_len: u16 LE] [payload bytes]
Response: [len: u32 LE] [JSON bytes]

Commands:
  0x01 GET_ATTESTATION  payload = nonce bytes  -> AttestationDoc JSON
  0x02 SIGN             payload = message bytes -> { signature, public_key }
```

Most Rust developers should use the `nautilus-enclave` library directly instead.

## Feature Flags

| Crate | Feature | Default | Purpose |
|-------|---------|---------|---------|
| `nautilus-cli` | `sui` | off | Enables on-chain commands. Adds `reqwest` dependency |
| `nautilus-cli` | `aws` | off | Enables EC2 enclave support check. Adds `aws-sdk-ec2` |
| `nautilus-enclave` | `nsm` | off | Enables real NSM device calls. Only works inside a Nitro Enclave |
| `nautilus-sidecar` | `nsm` | off | Passes through to `nautilus-enclave/nsm` |

## Running Tests

```bash
# All tests across all crates (uses mocks, no enclave needed)
cargo test

# Individual crates
cargo test -p nautilus-enclave      # 7 tests — crypto + attestation
cargo test -p nautilus-sidecar      # 3 tests — VSOCK protocol
cargo test -p nautilus-cli           # 12 tests — CLI subcommands
cargo test -p nautilus-cli --features sui  # includes config tests
```

## Environment Variables

| Variable | Used By | Description |
|----------|---------|-------------|
| `TEE_EC2_HOST` | `attest`, `register-enclave`, `verify-signature` | EC2 instance IP or hostname |
| `TEE_EC2_INSTANCE_ID` | `verify` | EC2 instance ID for enclave support check |
| `NAUTILUS_PACKAGE_ID` | `register-enclave`, `update-pcrs`, `verify-signature` | On-chain package ID |
| `NAUTILUS_CONFIG_ID` | `register-enclave`, `update-pcrs` | On-chain EnclaveConfig object ID |
| `NAUTILUS_CAP_ID` | `register-enclave`, `update-pcrs` | On-chain Cap object ID |
| `NAUTILUS_ENCLAVE_ID` | `verify-signature` | On-chain Enclave object ID |

## Related Repositories

| Repository | Description |
|-----------|-------------|
| [nautilus-tee-app](https://github.com/Ashwin-3cS/nautilus-tee-app/) | Reference TEE application — Axum sign-server powered by `nautilus-enclave`. Has `/sign_name`, `/get_attestation`, and `/health` endpoints. Clone and customize for your use case. |

## License

MIT
