import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, web3 } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import * as fs from "fs";
import * as path from "path";
import * as dotenv from "dotenv";

dotenv.config({ path: path.join(__dirname, "../.env") });

async function main() {
  const RPC = process.env.RPC || "https://rpc.mainnet.x1.xyz";
  const PROGRAM_ID = new PublicKey(process.env.PROGRAM_ID!);

  console.log("=== PURGE Fresh Deploy: Step 6 — Initialize Program ===");
  console.log("Program ID:", PROGRAM_ID.toBase58());
  console.log("RPC:", RPC);

  // Load wallet from default solana config
  const walletPath = process.env.WALLET_PATH ||
    path.join(process.env.HOME!, ".config/solana/id.json");
  const rawKey = JSON.parse(fs.readFileSync(walletPath, "utf-8"));
  const keypair = Keypair.fromSecretKey(new Uint8Array(rawKey));

  console.log("Wallet:", keypair.publicKey.toBase58());

  const connection = new Connection(RPC, "confirmed");
  const wallet = new anchor.Wallet(keypair);
  const provider = new AnchorProvider(connection, wallet, {
    commitment: "confirmed",
  });
  anchor.setProvider(provider);

  // Load IDL from build artifacts
  const idlPath = path.join(__dirname, "../target/idl/purge.json");
  if (!fs.existsSync(idlPath)) {
    throw new Error(
      "IDL not found. Run 'anchor build' first, then try again."
    );
  }
  const idl = JSON.parse(fs.readFileSync(idlPath, "utf-8"));

  const program = new Program(idl, provider);

  // Derive global_state PDA
  const [globalStatePDA] = PublicKey.findProgramAddressSync(
    [Buffer.from("global_state")],
    PROGRAM_ID
  );
  console.log("Global State PDA:", globalStatePDA.toBase58());

  // Require MINT_ADDRESS — needed to lock down ClaimMintReward
  if (!process.env.MINT_ADDRESS) {
    throw new Error("MINT_ADDRESS not set in .env. Run step 3 first.");
  }
  const mintAddress = new PublicKey(process.env.MINT_ADDRESS);
  console.log("Mint Address:", mintAddress.toBase58());

  // Check if already initialized
  const existing = await connection.getAccountInfo(globalStatePDA);
  if (existing) {
    console.log("✅ Global state already initialized. Nothing to do.");
    return;
  }

  console.log("Sending initialize transaction...");
  const tx = await (program.methods as any)
    .initialize(mintAddress)
    .accounts({
      globalState: globalStatePDA,
      authority: keypair.publicKey,
      systemProgram: web3.SystemProgram.programId,
    })
    .rpc();

  console.log("");
  console.log("✅ Program initialized!");
  console.log("TX:", tx);
  console.log("");
  console.log("PURGE is live on X1 mainnet.");
  console.log("Mint address:", process.env.MINT_ADDRESS);
  console.log("Program ID:  ", PROGRAM_ID.toBase58());
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
