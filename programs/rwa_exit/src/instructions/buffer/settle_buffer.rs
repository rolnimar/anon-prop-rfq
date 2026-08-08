use crate::constants::seeds;
use crate::instructions::buffer::{
    __client_accounts_buffer_accrual_accounts, __cpi_client_accounts_buffer_accrual_accounts,
    accounts::BufferAccrualAccountsBumps, accrue_buffer::accrue_buffer_from_accounts,
    BufferAccrualAccounts,
};
use crate::instructions::market_info::{load_main_offer, refresh_market_stats_pda};
use crate::state::State;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenInterface};

/// Accounts for a worker-triggered BUFFER settlement.
#[derive(Accounts)]
pub struct SettleBuffer<'info> {
    /// Global state containing worker, Rwa mint, and main-offer configuration.
    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = worker @ crate::ProtocolError::InvalidWorker,
        has_one = rwa_mint,
        constraint = !state.is_killed @ crate::ProtocolError::KillSwitchActivated,
        constraint = state.main_offer != Pubkey::default() @ crate::ProtocolError::InvalidMainOffer
    )]
    pub state: Box<Account<'info, State>>,

    /// Worker authorized to settle BUFFER and payer for market-stats initialization.
    #[account(mut)]
    pub worker: Signer<'info>,

    /// Rwa mint whose supply is increased by BUFFER accrual.
    #[account(mut)]
    pub rwa_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: PDA derivation and mint-authority ownership are validated here.
    #[account(
        seeds = [seeds::MINT_AUTHORITY],
        constraint = rwa_mint.mint_authority.unwrap() == mint_authority.key()
            @ crate::ProtocolError::NoMintAuthority,
        bump
    )]
    pub mint_authority: UncheckedAccount<'info>,

    /// Token program controlling the Rwa mint and BUFFER vault accounts.
    pub token_program: Interface<'info, TokenInterface>,

    /// System program used if market stats must be initialized.
    pub system_program: Program<'info, System>,

    /// CHECK: Parsed and validated against `state.main_offer` in instruction logic.
    pub main_offer: UncheckedAccount<'info>,

    /// BUFFER state and destination vault accounts.
    pub buffer_accounts: BufferAccrualAccounts<'info>,

    /// CHECK: Validated and optionally initialized by market-stats refresh.
    #[account(mut)]
    pub market_stats: UncheckedAccount<'info>,

    /// CHECK: PDA validation and loading are handled by market-stats refresh.
    pub circulating_supply_excluded_balance: UncheckedAccount<'info>,
}

/// Accrues BUFFER through the current timestamp and refreshes canonical market stats.
///
/// Only the configured worker may call this instruction. It provides an explicit
/// operational settlement path without requiring a trade or an unrelated mint.
pub fn settle_buffer(ctx: Context<SettleBuffer>) -> Result<()> {
    let offer = load_main_offer(
        ctx.program_id,
        &ctx.accounts.main_offer.to_account_info(),
        &ctx.accounts.state,
    )?;

    accrue_buffer_from_accounts(
        ctx.program_id,
        &ctx.accounts.state,
        &ctx.accounts.buffer_accounts,
        &offer,
        &ctx.accounts.rwa_mint,
        ctx.accounts.mint_authority.to_account_info(),
        ctx.bumps.mint_authority,
        &ctx.accounts.token_program,
    )?;

    ctx.accounts.rwa_mint.reload()?;
    refresh_market_stats_pda(
        &offer,
        &ctx.accounts.rwa_mint,
        &ctx.accounts
            .circulating_supply_excluded_balance
            .to_account_info(),
        &ctx.accounts.market_stats.to_account_info(),
        &ctx.accounts.worker.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        ctx.program_id,
    )?;

    Ok(())
}
