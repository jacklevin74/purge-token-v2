use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, MintTo};
use anchor_spl::associated_token::AssociatedToken;
use mpl_token_metadata::instructions::CreateMetadataAccountV3CpiBuilder;
use mpl_token_metadata::types::DataV2;

declare_id!("6K6md8GFmT8fncNbWqHSJrduYfG6HgnFCp34jdouGVSM");

pub const MAX_TERM_DAYS: u64 = 100;
pub const MIN_TERM_DAYS: u64 = 1;
pub const SECONDS_PER_DAY: u64 = 86400;
pub const INITIAL_AMP: u64 = 69;  // AMP starts at 69, drops by 1 each day, floor 0
pub const TOKEN_DECIMALS: u32 = 2;  // Mint has 2 decimals. 69 × 1 × 10^2 = 6900 raw = 69.00 PURGE.

#[program]
pub mod purge {
    use super::*;

    /// Initialize global state — called once at deployment
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let global_state = &mut ctx.accounts.global_state;
        global_state.total_minters = 0;
        global_state.total_x_burnt = 0;
        global_state.active_mints = 0;
        global_state.genesis_ts = Clock::get()?.unix_timestamp;
        global_state.bump = ctx.bumps.global_state;

        msg!("PURGE initialized. Genesis: {}", global_state.genesis_ts);
        Ok(())
    }

    /// Attach Metaplex metadata to the PURGE mint — PDA signs as mint authority.
    /// Call once after deployment.
    pub fn create_metadata(
        ctx: Context<CreateMetadata>,
        name: String,
        symbol: String,
        uri: String,
    ) -> Result<()> {
        let seeds = &[b"mint_authority".as_ref(), &[ctx.bumps.mint_authority]];
        let signer = &[&seeds[..]];

        CreateMetadataAccountV3CpiBuilder::new(&ctx.accounts.token_metadata_program)
            .metadata(&ctx.accounts.metadata)
            .mint(&ctx.accounts.mint.to_account_info())
            .mint_authority(&ctx.accounts.mint_authority)
            .payer(&ctx.accounts.payer)
            .update_authority(&ctx.accounts.payer, true)
            .system_program(&ctx.accounts.system_program)
            .data(DataV2 {
                name,
                symbol,
                uri,
                seller_fee_basis_points: 0,
                creators: None,
                collection: None,
                uses: None,
            })
            .is_mutable(true)
            .invoke_signed(signer)?;

        msg!("Metadata attached to PURGE mint.");
        Ok(())
    }

    /// Claim rank — start a new mint slot with a specified term.
    /// slot_index must equal the user's current next_slot_index (sequential, no gaps).
    /// No upper limit on how many times a wallet can claim — governed only by rent + gas.
    pub fn claim_rank(ctx: Context<ClaimRank>, term_days: u64, slot_index: u32) -> Result<()> {
        require!(
            term_days >= MIN_TERM_DAYS && term_days <= MAX_TERM_DAYS,
            PurgeError::InvalidTerm
        );

        let counter = &mut ctx.accounts.user_counter;
        let user_mint = &mut ctx.accounts.user_mint;
        let global_state = &mut ctx.accounts.global_state;
        let current_ts = Clock::get()?.unix_timestamp;

        // Enforce sequential slot allocation — prevents gaps and replay
        require!(slot_index == counter.next_slot_index, PurgeError::InvalidSlotIndex);

        let maturity_ts = current_ts + (term_days as i64 * SECONDS_PER_DAY as i64);

        // Snapshot AMP at claim time — locked in for this slot regardless of future decay
        let days_since_genesis =
            ((current_ts - global_state.genesis_ts) / SECONDS_PER_DAY as i64).max(0) as u64;
        let amp_at_claim = INITIAL_AMP.saturating_sub(days_since_genesis);

        user_mint.owner = ctx.accounts.user.key();
        user_mint.slot_index = slot_index;
        user_mint.term_days = term_days;
        user_mint.mature_ts = maturity_ts;
        user_mint.claimed = false;
        user_mint.rank = global_state.total_minters + 1;
        user_mint.amp_snapshot = amp_at_claim;
        user_mint.reward_amount = 0;
        user_mint.bump = ctx.bumps.user_mint;

        counter.next_slot_index += 1;
        counter.active_mints = counter.active_mints.saturating_add(1);
        counter.bump = ctx.bumps.user_counter;

        global_state.total_minters += 1;
        global_state.active_mints = global_state.active_mints.saturating_add(1);

        msg!(
            "Rank claimed: slot={}, rank={}, term={} days, amp={}, matures={}",
            slot_index, user_mint.rank, term_days, amp_at_claim, maturity_ts
        );
        Ok(())
    }

    /// Claim mint reward for a specific slot after maturity
    pub fn claim_mint_reward(ctx: Context<ClaimMintReward>, slot_index: u32) -> Result<()> {
        let user_mint = &mut ctx.accounts.user_mint;
        let global_state = &mut ctx.accounts.global_state;
        let counter = &mut ctx.accounts.user_counter;
        let current_ts = Clock::get()?.unix_timestamp;

        require!(!user_mint.claimed, PurgeError::AlreadyClaimed);
        require!(current_ts >= user_mint.mature_ts, PurgeError::NotMature);

        let reward = calculate_reward(
            user_mint.amp_snapshot,
            user_mint.term_days,
        )?;

        // Mint tokens — PDA signs as mint authority
        let seeds = &[b"mint_authority".as_ref(), &[ctx.bumps.mint_authority]];
        let signer = &[&seeds[..]];

        let cpi_accounts = MintTo {
            mint: ctx.accounts.mint.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: ctx.accounts.mint_authority.to_account_info(),
        };
        token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                cpi_accounts,
                signer,
            ),
            reward,
        )?;

        user_mint.claimed = true;
        user_mint.reward_amount = reward;

        counter.active_mints = counter.active_mints.saturating_sub(1);
        global_state.active_mints = global_state.active_mints.saturating_sub(1);

        msg!("Slot {} claimed: {} raw PURGE (2 dec)", slot_index, reward);
        Ok(())
    }
}

/// Reward = AMP_at_claim × term_days × 10^18
///
/// AMP starts at 69 on day 0, drops by 1 each day, floors at 0 on day 69+.
/// AMP is snapshotted at claim_rank time and stored in the slot — so latecomers
/// pay the penalty at claim time, not at maturity.
///
/// Max case: AMP=69, term=100 → 6,900 PURGE (6900 × 10^18 raw)
/// This exceeds u64::MAX (~1.8×10^19), so we use u128 throughout and
/// return Result<u64> — failing explicitly rather than silently truncating.
///
/// Example:
///   Day 0, 100-day term  → 69 × 100 × 10^18 raw = 6,900 PURGE
///   Day 10, 50-day term  → 59 × 50  × 10^18 raw = 2,950 PURGE
///   Day 69+, any term    → 0 PURGE (AMP fully decayed)
fn calculate_reward(amp_snapshot: u64, term_days: u64) -> Result<u64> {
    if amp_snapshot == 0 || term_days == 0 {
        return Ok(0);
    }

    // Cap amp at INITIAL_AMP — guards against legacy slots
    let amp = amp_snapshot.min(INITIAL_AMP);

    // All math in u128 to prevent intermediate overflow.
    // Max: 69 × 100 × 10^18 = 6.9×10^21 — exceeds u64::MAX (1.84×10^19).
    // We saturate at the largest whole-PURGE multiple that fits in u64:
    //   floor(u64::MAX / 10^18) × 10^18 = 18 × 10^18 = 18 PURGE.
    // This avoids SPL token program overflow on mint_to.
    let scale = 10u128.pow(TOKEN_DECIMALS);
    let max_safe = (u64::MAX as u128 / scale) * scale; // largest u64 that is a multiple of scale
    let base = (amp as u128) * (term_days as u128);
    let scaled = base * scale;
    Ok(scaled.min(max_safe) as u64)
}

// ─── Account Contexts ────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct CreateMetadata<'info> {
    #[account(mut)]
    pub mint: Account<'info, Mint>,

    /// CHECK: PDA — signs as mint authority
    #[account(seeds = [b"mint_authority"], bump)]
    pub mint_authority: UncheckedAccount<'info>,

    /// CHECK: Metaplex metadata PDA — validated by mpl program
    #[account(mut)]
    pub metadata: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,

    /// CHECK: Metaplex token metadata program
    #[account(address = mpl_token_metadata::ID)]
    pub token_metadata_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + GlobalState::SIZE,
        seeds = [b"global_state"],
        bump
    )]
    pub global_state: Account<'info, GlobalState>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(term_days: u64, slot_index: u32)]
pub struct ClaimRank<'info> {
    /// Per-user counter — one PDA per wallet, tracks next_slot_index
    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserCounter::SIZE,
        seeds = [b"user_counter", user.key().as_ref()],
        bump
    )]
    pub user_counter: Account<'info, UserCounter>,

    /// Per-slot PDA — unique position keyed by (user, slot_index)
    #[account(
        init,
        payer = user,
        space = 8 + UserMint::SIZE,
        seeds = [b"user_mint", user.key().as_ref(), &slot_index.to_le_bytes()],
        bump
    )]
    pub user_mint: Account<'info, UserMint>,

    #[account(mut, seeds = [b"global_state"], bump = global_state.bump)]
    pub global_state: Account<'info, GlobalState>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(slot_index: u32)]
pub struct ClaimMintReward<'info> {
    #[account(
        mut,
        seeds = [b"user_counter", user.key().as_ref()],
        bump = user_counter.bump,
    )]
    pub user_counter: Account<'info, UserCounter>,

    #[account(
        mut,
        seeds = [b"user_mint", user.key().as_ref(), &slot_index.to_le_bytes()],
        bump = user_mint.bump,
        constraint = user_mint.owner == user.key() @ PurgeError::Unauthorized,
    )]
    pub user_mint: Account<'info, UserMint>,

    #[account(mut, seeds = [b"global_state"], bump = global_state.bump)]
    pub global_state: Account<'info, GlobalState>,

    #[account(mut)]
    pub mint: Account<'info, Mint>,

    /// CHECK: PDA used as mint authority — verified by seeds
    #[account(seeds = [b"mint_authority"], bump)]
    pub mint_authority: UncheckedAccount<'info>,

    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint,
        associated_token::authority = user,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

// ─── Account Structs ─────────────────────────────────────────────────────────

#[account]
pub struct GlobalState {
    pub total_minters: u64,
    pub total_x_burnt: u64,   // reserved for future burn mechanics
    pub active_mints: u64,
    pub genesis_ts: i64,
    pub bump: u8,
}

impl GlobalState {
    pub const SIZE: usize = 8 + 8 + 8 + 8 + 1;
}

/// One PDA per wallet — tracks slot allocation
#[account]
pub struct UserCounter {
    pub next_slot_index: u32,  // monotonically increasing — next slot to allocate
    pub active_mints: u32,     // u32: 4 billion concurrent mints per wallet (effectively unlimited)
    pub bump: u8,
}

impl UserCounter {
    pub const SIZE: usize = 4 + 4 + 1;
}

/// One PDA per (wallet, slot_index) — individual mint position
#[account]
pub struct UserMint {
    pub owner: Pubkey,
    pub slot_index: u32,
    pub term_days: u64,
    pub mature_ts: i64,
    pub claimed: bool,
    pub rank: u64,
    pub amp_snapshot: u64,    // AMP value locked at claim_rank time
    pub reward_amount: u64,
    pub bump: u8,
}

impl UserMint {
    pub const SIZE: usize = 32 + 4 + 8 + 8 + 1 + 8 + 8 + 8 + 1;
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[error_code]
pub enum PurgeError {
    #[msg("Invalid term. Must be 1–100 days.")]
    InvalidTerm,
    #[msg("slot_index must equal next_slot_index for this wallet.")]
    InvalidSlotIndex,
    #[msg("Reward already claimed.")]
    AlreadyClaimed,
    #[msg("Mint has not matured yet.")]
    NotMature,
    #[msg("Unauthorized.")]
    Unauthorized,
    #[msg("Reward calculation overflow — contact protocol team.")]
    RewardOverflow,
}
