use crate::constants::seeds;
use crate::instructions::buffer::accounts::{
    __client_accounts_buffer_accrual_accounts, __cpi_client_accounts_buffer_accrual_accounts,
    BufferAccrualAccountsBumps,
};
use crate::instructions::buffer::{
    accrue_buffer::accrue_buffer_from_accounts, BufferAccrualAccounts,
    BufferGrossYieldUpdatedEvent, MAX_BUFFER_GROSS_APR,
};
use crate::instructions::market_info::market_stats::refresh_market_stats_pda;
use crate::instructions::Offer;
use crate::state::State;
use anchor_lang::solana_program::program_option::COption;
use anchor_lang::{prelude::*, Accounts};
use anchor_spl::token_interface::{Mint, TokenInterface};

#[derive(Accounts)]
pub struct SetBufferGrossYield<'info> {
    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = boss,
        has_one = rwa_mint,
        constraint = !state.is_killed @ crate::ProtocolError::KillSwitchActivated
    )]
    pub state: Box<Account<'info, State>>,

    #[account(mut)]
    pub boss: Signer<'info>,

    #[account(address = state.main_offer @ crate::ProtocolError::InvalidMainOffer)]
    pub main_offer: AccountLoader<'info, Offer>,

    #[account(mut)]
    pub rwa_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: PDA derivation is validated by seeds constraint.
    #[account(seeds = [seeds::OFFER_VAULT_AUTHORITY], bump)]
    pub offer_vault_authority: UncheckedAccount<'info>,

    /// CHECK: PDA derivation is validated by seeds constraint.
    #[account(
        seeds = [seeds::MINT_AUTHORITY],
        constraint = rwa_mint.mint_authority == COption::Some(mint_authority.key()) @ crate::ProtocolError::NoMintAuthority,
        bump
    )]
    pub mint_authority: UncheckedAccount<'info>,

    pub buffer_accounts: BufferAccrualAccounts<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,

    /// CHECK: PDA derivation is validated by seeds constraint and the account is optionally initialized in settlement helper.
    #[account(mut, seeds = [seeds::MARKET_STATS], bump)]
    pub market_stats: UncheckedAccount<'info>,

    /// CHECK: PDA derivation is validated by seeds constraint; data loading is handled by market stats refresh.
    #[account(seeds = [seeds::CIRCULATING_SUPPLY_EXCLUDED_BALANCE], bump)]
    pub circulating_supply_excluded_balance: UncheckedAccount<'info>,
}

pub fn set_buffer_gross_apr(ctx: Context<SetBufferGrossYield>, gross_yield: u64) -> Result<()> {
    require!(
        gross_yield <= MAX_BUFFER_GROSS_APR,
        crate::ProtocolError::InvalidAPR
    );

    let mut buffer_state = ctx.accounts.buffer_accounts.load_buffer_state()?;

    require!(
        buffer_state.gross_apr != gross_yield,
        crate::ProtocolError::NoChange
    );

    let offer = ctx.accounts.main_offer.load()?;
    require_keys_eq!(
        ctx.accounts.rwa_mint.key(),
        offer.token_out_mint,
        crate::ProtocolError::InvalidTokenOutMint
    );
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
        &ctx.accounts.boss.to_account_info(),
        &ctx.accounts.system_program.to_account_info(),
        ctx.program_id,
    )?;

    buffer_state = ctx.accounts.buffer_accounts.load_buffer_state()?;
    buffer_state.gross_apr = gross_yield;
    ctx.accounts
        .buffer_accounts
        .store_buffer_state(&buffer_state)?;

    emit!(BufferGrossYieldUpdatedEvent { gross_yield });

    Ok(())
}
