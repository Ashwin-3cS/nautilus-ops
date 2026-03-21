# Nautilus-Ops

Self-managed TEE orchestrator for AWS Nitro Enclaves on the Sui blockchain. Build enclave images, deploy them to EC2, register attestations on-chain, and verify enclave signatures — all from one CLI.

Supports multiple template types:
- **Rust** — [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) using the `nautilus-enclave` library
- **TypeScript** — [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) (fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts)) using Bun + argonaut

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
│  build               │───────────────>│  ┌────────────────────────────┐  │
│  init-ci             │                │  │   Nitro Enclave (isolated)  │  │
│  deploy-contract     │                │  │                            │  │
│  update-pcrs         │                │  │   Your TEE App             │  │
│  register-enclave    │                │  │   (Rust / TS / any lang)   │  │
│  verify-signature    │                │  │   Ed25519 keygen + sign    │  │
│  attest              │                │  │   NSM attestation          │  │
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

| Aspect | Rust Template | TS Template |
|--------|--------------|-------------|
| Repo | [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) | [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) (fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts)) |
| Default HTTP port | 4000 | 3000 |
| Attestation endpoint | `GET /get_attestation` | `GET /attestation` |
| Sign endpoint | `POST /sign_name` (JSON body) | `POST /sign` (raw bytes) |
| VSOCK bridge | socat (systemd services) | argonaut host binary |
| Build | `docker build` + `nitro-cli build-enclave` | `make` (docker multi-stage, EIF built inside) |
| On-chain verify | `verify_signed_name` (BCS + IntentMessage) | `verify_signed_data` (blake2b256 + raw bytes) |
| Auto-detection | `Cargo.toml` in project root | `argonaut/` dir + `package.json` |

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
│       ├── main.rs                # Clap entry point — 8 subcommands
│       ├── build.rs               # nautilus build — Docker + nitro-cli -> .eif + PCRs
│       ├── init_ci.rs             # nautilus init-ci — generates GitHub Actions workflow
│       ├── attest.rs              # nautilus attest — fetch attestation + parse CBOR
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

### Mock support

By default (without the `nsm` feature), all NSM calls return deterministic mock data. Your app compiles and runs on your laptop with the same code that runs inside the enclave. No conditional compilation needed in your app code.

### Reference implementations

| Template | Repository | Description |
|----------|-----------|-------------|
| Rust | [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) | Axum sign-server with `/sign_name`, `/get_attestation`, `/health`. Uses `nautilus-enclave` directly |
| TypeScript | [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) | Bun + argonaut framework with `/sign`, `/attestation`, `/health_check`. Fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts) |

---

## For dApp Developers — CLI Workflow

Once you have a TEE app running inside an enclave, use the CLI to manage the full on-chain lifecycle.

### Full End-to-End Flow

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
# Template is auto-detected (Cargo.toml -> Rust, argonaut/ + package.json -> TS)

# Set GitHub secrets: TEE_EC2_HOST, TEE_EC2_USER, TEE_EC2_SSH_KEY
# Push to main -> enclave deploys automatically
```

**Step 3: Deploy the Smart Contract**

```bash
nautilus deploy-contract --network testnet
# Publishes contracts/nautilus/ to Sui
# Saves package_id, config_object_id, cap_object_id to .nautilus.toml
```

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

# TS template — signs blake2b256(raw_data) directly
nautilus verify-signature \
  --template ts \
  --host <EC2_IP> \
  --enclave-id <ENCLAVE_OBJECT_ID> \
  --data "Nautilus"
```

### What happens after setup

After steps 1–5, any dApp on Sui can call `verify_signature()` or `verify_signed_data()` in their Move contract to verify that a payload was signed by your attested enclave. The CLI is only needed for setup and management — verification is fully on-chain and permissionless.

---

## CLI Reference

| Command | Description | Requires |
|---------|-------------|----------|
| `nautilus build` | Build `.eif` from Dockerfile, extract PCR measurements | Docker |
| `nautilus init-ci` | Generate GitHub Actions deployment workflow | — |
| `nautilus attest` | Fetch attestation from enclave, parse CBOR, extract PCRs | Enclave running |
| `nautilus verify` | Check if an EC2 instance supports Nitro Enclaves | `--features aws` |
| `nautilus deploy-contract` | Publish the Move contract to Sui | `--features sui`, Sui CLI |
| `nautilus update-pcrs` | Set expected PCR values in `EnclaveConfig` | `--features sui`, Sui CLI |
| `nautilus register-enclave` | Register enclave on-chain with attestation | `--features sui`, Sui CLI |
| `nautilus verify-signature` | Verify an enclave signature on-chain | `--features sui`, Sui CLI |

Run `nautilus <command> --help` for full flag details. Use `--template rust|ts` to override auto-detection.

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

**TS template — blake2b256 + raw bytes:**
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
cargo test -p nautilus-cli                        # 27 tests — CLI, config, build, init-ci
cargo test -p nautilus-cli --features sui         # includes on-chain config tests
```

## Related Repositories

| Repository | Description |
|-----------|-------------|
| [nautilus-rust](https://github.com/Ashwin-3cS/nautilus-rust/) | Rust TEE template — Axum sign-server powered by `nautilus-enclave`. Endpoints: `/sign_name`, `/get_attestation`, `/health` |
| [nautilus-ts](https://github.com/Ashwin-3cS/nautilus-ts/) | TypeScript TEE template — Bun + argonaut framework. Fork of [unconfirmedlabs/nautilus-ts](https://github.com/unconfirmedlabs/nautilus-ts). Endpoints: `/sign`, `/attestation`, `/health_check` |

## License

MIT
