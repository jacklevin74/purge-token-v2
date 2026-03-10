# PURGE Token — Fresh Deploy Guide

Run these in order. Each script picks up where the last left off.

## Prerequisites

```bash
anchor --version    # need 0.30.1
solana --version
spl-token --version
cargo install metaboss   # only needed for step 5
```

---

## Step 1 — Generate program keypair + patch code

```bash
bash scripts/1_update_program_id.sh
```

Generates `program-keypair.json`, patches `declare_id!` in `lib.rs` and `Anchor.toml`.

---

## Step 2 — Build and deploy

```bash
bash scripts/2_build_and_deploy.sh
```

Runs `anchor build` then `anchor deploy` to X1 mainnet.

---

## Step 3 — Create the token mint

```bash
bash scripts/3_create_mint.sh
```

Derives the mint authority PDA, creates the SPL token with 18 decimals.
Saves everything to `.env`.

---

## Step 4 — Upload metadata

First, upload your logo image to [Pinata](https://pinata.cloud) or [nft.storage](https://nft.storage) and get the URL.

```bash
bash scripts/4_upload_metadata.sh https://YOUR_IMAGE_URL_HERE
```

Builds `purge-metadata.json` and uploads it if `NFT_STORAGE_API_KEY` is set.
Otherwise it writes the JSON and tells you to upload manually.

---

## Step 5 — Attach metadata to mint

```bash
bash scripts/5_attach_metadata.sh https://YOUR_METADATA_URI_HERE
```

Uses `metaboss` to attach name/symbol/image to the mint on-chain.

---

## Step 6 — Initialize the program

```bash
bash scripts/6_initialize_program.sh
```

Calls the `initialize` instruction — sets genesis timestamp, creates global state PDA.
PURGE is live after this.

---

## What gets created

| Thing | Where |
|---|---|
| Program keypair | `program-keypair.json` |
| Program ID | Printed in step 1, stored in `.env` |
| Mint address | Printed in step 3, stored in `.env` |
| Mint authority | PDA: seed `"mint_authority"` + program ID |
| Global state | PDA: seed `"global_state"` + program ID |
| Metadata | On-chain via Metaplex |

---

## After deploy — verify

```bash
source .env
solana program show $PROGRAM_ID --url $RPC
spl-token display $MINT_ADDRESS --url $RPC
```
