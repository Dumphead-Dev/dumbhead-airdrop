use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Mint, Transfer};

declare_id!("BXr1vqG1n44t7v2nigJCo1tcTuS7w2FECCq35RL1zKBy");

// lol this whole program: single-use init, then everyone can claim based on bot-generated CSV snapshot
#[program]
pub mod airdrop {
    use super::*;

    // only run ONCE, right after snapshot done. locks all params.
    pub fn initialize(
        ctx: Context<Initialize>,
        snapshot_hash: [u8; 32], // hash of the "airdrop.csv" file. makes auditing easy
        claim_start_ts: i64,     // unix timestamp. don't open until you're ready
        claim_duration: i64,     // claim window (should be 60 days: 60*24*60*60)
    ) -> Result<()> {
        let state = &mut ctx.accounts.state;
        state.authority = *ctx.accounts.authority.key;
        state.snapshot_hash = snapshot_hash;
        state.claim_start_ts = claim_start_ts;
        state.claim_duration = claim_duration;
        state.claim_closed = false;
        msg!("init done, claim for {} days, hash={:?}", claim_duration / 86400, snapshot_hash);
        Ok(())
    }

    // the only other instruction: claim() (anyone with a record can call)
    pub fn claim(ctx: Context<Claim>) -> Result<()> {
        let state = &ctx.accounts.state;
        let claim = &mut ctx.accounts.claim;

        // time check: can't claim before window, can't claim after
        let clock = Clock::get()?;
        let now = clock.unix_timestamp;
        if now < state.claim_start_ts || now > state.claim_start_ts + state.claim_duration {
            // claim window not open/closed
            return err!(ErrorCode::ClaimWindowClosed);
        }
        // already claimed? one shot, no do-overs
        if claim.claimed {
            return err!(ErrorCode::AlreadyClaimed);
        }
        // amount sanity
        require!(claim.amount > 0, ErrorCode::NoAllocation);

        // SPL transfer -- send from the main vault to user's token account (they pay rent+fees)
        let seeds: &[&[u8]] = &[b"vault", state.snapshot_hash.as_ref()];
        let signer_seeds: &[&[&[u8]]] = &[seeds];
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.user_ata.to_account_info(),
                authority: ctx.accounts.vault_auth.to_account_info(),
            },
            signer_seeds,
        );
        // do the thing
        token::transfer(cpi_ctx, claim.amount)?;

        // mark claimed
        claim.claimed = true;
        claim.claimed_at = Some(now);

        // log for the chain
        msg!("airdrop: {} claimed {}", claim.wallet, claim.amount);

        Ok(())
    }
}

// all the program params (locked at init, no upgrades, so transparent)
#[account]
pub struct State {
    pub authority: Pubkey,       // creator (audit only)
    pub snapshot_hash: [u8; 32], // CSV hash
    pub claim_start_ts: i64,     // unix timestamp
    pub claim_duration: i64,     // seconds
    pub claim_closed: bool,      // not used yet, set to true if 60d pass
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = authority, space = 8 + State::LEN)]
    pub state: Account<'info, State>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// space, man. solana rent ain't free.
impl State {
    pub const LEN: usize = 32 + 32 + 8 + 8 + 1;
}

// one record per winner (all based on bot’s CSV, don't fudge)
#[account]
pub struct Claim {
    pub wallet: Pubkey,         // who can claim this record
    pub amount: u64,            // their allocation (final, precomputed)
    pub claimed: bool,          // default false, set true after claim
    pub claimed_at: Option<i64> // for audit, unix timestamp
}
// 32 + 8 + 1 + 9 (Option<i64>) = 50 bytes
impl Claim {
    pub const LEN: usize = 32 + 8 + 1 + 9;
}

// PDA for each claim. will fill in batch at first deploy.
#[derive(Accounts)]
pub struct Claim<'info> {
    #[account(mut, has_one = state)]
    pub claim: Account<'info, Claim>,
    #[account()]
    pub state: Account<'info, State>,
    #[account(mut, signer)]
    pub wallet: Signer<'info>,

    // SPL: vault authority, vault, user's ATA
    #[account(seeds = [b"vault", state.snapshot_hash.as_ref()], bump)]
    pub vault_auth: AccountInfo<'info>,
    #[account(mut, token::mint = mint, token::authority = vault_auth)]
    pub vault: Account<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::authority = wallet)]
    pub user_ata: Account<'info, TokenAccount>,
    pub mint: Account<'info, Mint>,
    pub token_program: Program<'info, Token>,
}

// error codes: plain, just for debugging chain-side
#[error_code]
pub enum ErrorCode {
    #[msg("Airdrop claim window is not open.")]
    ClaimWindowClosed,
    #[msg("Airdrop already claimed.")]
    AlreadyClaimed,
    #[msg("No allocation, sorry.")]
    NoAllocation,
}
