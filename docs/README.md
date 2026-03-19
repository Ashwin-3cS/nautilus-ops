# Nautilus-Ops

Self-managed TEE orchestrator for AWS Nitro Enclaves on the Sui blockchain. Build enclave images, deploy them to EC2, register attestations on-chain, and verify enclave signatures — all from one CLI.

## Architecture

```
Developer Machine                       EC2 Instance (Nitro-enabled)
┌─────────────────────┐                ┌──────────────────────────────────┐
│  nautilus CLI        │                │  socat bridge (TCP:4000 <-> VSOCK)│
│                      │  SSH / HTTP    │                                  │
│  build               │───────────────>│  ┌────────────────────────────┐  │
│  init-ci             │                │  │   Nitro Enclave (isolated)  │  │
│  deploy-contract     │                │  │                            │  │
│  update-pcrs         │                │  │   sign-server (Axum :4000) │  │
│  register-enclave    │                │  │   - Ed25519 keygen (NSM)   │  │
│  verify-signature    │                │  │   - /sign_name             │  │
│                      │                │  │   - /get_attestation       │  │
└─────────────────────┘                │  └────────────────────────────┘  │
         │                              └──────────────────────────────────┘
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
├── nautilus-sidecar/              # Enclave binary (runs inside Nitro Enclave)
│   └── src/
│       ├── main.rs                # Boot: keygen -> VSOCK server on port 5000
│       ├── crypto.rs              # Ed25519 keygen, sign, verify
│       ├── nsm.rs                 # NSM attestation (real or mock)
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
- **AWS CLI** (optional) — only for `nautilus verify` with `--features aws`

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

# With AWS EC2 verification
cargo install --path nautilus-cli --features aws

# All features
cargo install --path nautilus-cli --features "sui,aws"
```

## Quick Start — Full End-to-End Flow

### 1. Build the Enclave Image

```bash
cd /path/to/your-tee-app

nautilus build -f Containerfile -o out/enclave.eif
# Outputs: out/enclave.eif + out/enclave.eif.pcrs.json
```

### 2. Deploy to EC2 via CI

```bash
nautilus init-ci --cpu-count 2 --memory-mib 4096 -f Containerfile
# Creates .github/workflows/nautilus-deploy.yml

# Set GitHub secrets: TEE_EC2_HOST, TEE_EC2_USER, TEE_EC2_SSH_KEY
# Push to main -> enclave deploys automatically
```

### 3. Deploy the Smart Contract

```bash
cd /path/to/nautilus-ops

nautilus deploy-contract --network testnet
# Publishes contracts/nautilus/ to Sui
# Saves package_id, config_object_id, cap_object_id to .nautilus.toml
```

### 4. Update PCRs

From a PCR file (output of `nautilus build`):

```bash
nautilus update-pcrs --pcr-file out/enclave.eif.pcrs.json
```

Or pass them directly:

```bash
nautilus update-pcrs \
  --pcr0 "13172639e463cc74..." \
  --pcr1 "13172639e463cc74..." \
  --pcr2 "21b9efbc18480766..."
```

### 5. Register the Enclave On-Chain

```bash
nautilus register-enclave --host <EC2_IP>
# Fetches live attestation from the enclave
# Verifies AWS Nitro root CA chain on-chain
# Creates an Enclave object with the verified Ed25519 public key
```

Or combine steps 4 + 5 in one command:

```bash
nautilus register-enclave --host <EC2_IP> --pcr-file out/enclave.eif.pcrs.json
```

### 6. Verify a Signature On-Chain

```bash
nautilus verify-signature \
  --host <EC2_IP> \
  --enclave-id <ENCLAVE_OBJECT_ID> \
  --name "Alice"
# Calls /sign_name on the enclave
# Submits the signature to on-chain verify_signed_name()
# Transaction succeeds only if the signature is valid
```

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
package_id = "0x4a5174f8..."
config_object_id = "0xd325f520..."
cap_object_id = "0xf0224251..."
```

All on-chain commands (`register-enclave`, `update-pcrs`, `verify-signature`) auto-read these values. You can override any value with CLI flags or environment variables:

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

## Sidecar (Enclave Binary)

The `nautilus-sidecar` crate runs inside the Nitro Enclave. It generates an Ed25519 keypair on boot and listens on VSOCK port 5000 with a binary protocol:

```
Request:  [cmd: u8] [payload_len: u16 LE] [payload bytes]
Response: [len: u32 LE] [JSON bytes]

Commands:
  0x01 GET_ATTESTATION  payload = nonce bytes  -> AttestationDoc JSON
  0x02 SIGN             payload = message bytes -> { signature, public_key }
```

Build for local testing (mock NSM):

```bash
cargo build -p nautilus-sidecar
```

Build for deployment inside a real enclave:

```bash
cargo build -p nautilus-sidecar --features nsm --target x86_64-unknown-linux-musl --release
```

## Feature Flags

| Crate | Feature | Default | Purpose |
|-------|---------|---------|---------|
| `nautilus-cli` | `sui` | off | Enables on-chain commands (deploy-contract, register-enclave, update-pcrs, verify-signature). Adds `reqwest` dependency |
| `nautilus-cli` | `aws` | off | Enables `nautilus verify` (EC2 enclave support check). Adds `aws-sdk-ec2` (~200MB) |
| `nautilus-sidecar` | `nsm` | off | Enables real NSM device calls. Only works inside a Nitro Enclave on Linux |

## Running Tests

```bash
# All tests (no feature flags needed — uses mocks)
cargo test

# CLI tests only
cargo test -p nautilus-cli

# With Sui features
cargo test -p nautilus-cli --features sui

# Sidecar tests
cargo test -p nautilus-sidecar
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

## License

MIT
