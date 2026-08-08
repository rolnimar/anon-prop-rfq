use crate::constants::seeds;
use crate::instructions::market_info::offer_valuation_utils::get_active_vector_and_current_price;
use crate::instructions::market_info::{
    calculate_circulating_supply, calculate_tvl as calculate_shared_tvl,
    load_circulating_supply_excluded_balance_amount,
};
use crate::instructions::Offer;
use crate::state::State;
use crate::utils::read_optional_token_account_amount;

use anchor_lang::prelude::*;
use anchor_lang::Accounts;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_interface::{Mint, TokenInterface};

/// Event emitted when TVL (Total Value Locked) calculation is completed
///
/// Provides transparency for tracking total value metrics for offers.
#[event]
pub struct GetTVLEvent {
    /// The PDA address of the offer for which TVL was calculated
    pub offer_pda: Pubkey,
    /// Calculated TVL in base units (circulating_supply * current_price / 10^9)
    pub tvl: u64,
    /// Current price with scale=9 used for TVL calculation
    pub current_price: u64,
    /// Circulating token supply (total_supply - excluded balances) in base units
    pub token_supply: u64,
    /// Unix timestamp when the TVL calculation was performed
    pub timestamp: u64,
}

/// Account structure for querying TVL (Total Value Locked) information
///
/// This struct defines the accounts required to calculate the TVL for a specific
/// offer by combining current pricing with circulating token supply. The calculation
/// is read-only and validates all accounts belong to the same offer.
#[derive(Accounts)]
pub struct GetTVL<'info> {
    /// The offer account containing pricing vectors for current price calculation
    ///
    /// This account is validated as a PDA derived from token mint addresses
    /// and contains time-based pricing vectors for TVL calculation.
    #[account(
        seeds = [
            seeds::OFFER,
            token_in_mint.key().as_ref(),
            token_out_mint.key().as_ref()
        ],
        bump = offer.load()?.bump
    )]
    pub offer: AccountLoader<'info, Offer>,

    /// The input token mint account for offer validation
    #[account(
        constraint =
            token_in_mint.key() == offer.load()?.token_in_mint
            @ crate::ProtocolError::InvalidTokenInMint
    )]
    pub token_in_mint: InterfaceAccount<'info, Mint>,

    /// The output token mint account containing total supply information
    #[account(
        constraint =
            token_out_mint.key() == offer.load()?.token_out_mint
            @ crate::ProtocolError::InvalidTokenOutMint
    )]
    pub token_out_mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA derivation is validated by seeds constraint
    #[account(seeds = [seeds::OFFER_VAULT_AUTHORITY], bump)]
    pub vault_authority: UncheckedAccount<'info>,

    /// CHECK: Account address is validated by the constraint below to allow passing uninitialized vault account
    #[account(
        constraint = vault_token_out_account.key()
            == get_associated_token_address_with_program_id(
                &vault_authority.key(),
                &token_out_mint.key(),
                &token_out_program.key(),
            ) @ crate::ProtocolError::InvalidVaultAccount
    )]
    pub vault_token_out_account: UncheckedAccount<'info>,

    pub token_out_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct GetTVLV2<'info> {
    #[account(
        seeds = [
            seeds::OFFER,
            token_in_mint.key().as_ref(),
            token_out_mint.key().as_ref()
        ],
        bump = offer.load()?.bump
    )]
    pub offer: AccountLoader<'info, Offer>,

    #[account(
        constraint =
            token_in_mint.key() == offer.load()?.token_in_mint
            @ crate::ProtocolError::InvalidTokenInMint
    )]
    pub token_in_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint =
            token_out_mint.key() == offer.load()?.token_out_mint
            @ crate::ProtocolError::InvalidTokenOutMint
    )]
    pub token_out_mint: InterfaceAccount<'info, Mint>,

    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        constraint = token_out_mint.key() == state.rwa_mint
            @ crate::ProtocolError::InvalidRwaMint
    )]
    pub state: Box<Account<'info, State>>,

    /// CHECK: PDA derivation is validated by seeds constraint; uninitialized data means zero.
    #[account(seeds = [seeds::CIRCULATING_SUPPLY_EXCLUDED_BALANCE], bump)]
    pub circulating_supply_excluded_balance: UncheckedAccount<'info>,
}

/// Calculates and returns the current TVL (Total Value Locked) for a specific offer
///
/// This read-only instruction calculates the TVL by combining the current NAV price
/// with the circulating token supply. The calculation excludes offer-vault
/// token holdings from total supply to represent tokens in circulation.
///
/// Formula: `TVL = circulating_supply * current_price / 10^9`
///
/// The calculation uses the current active pricing vector to determine NAV and
/// subtracts the offer-vault balance from total supply to get circulating supply.
///
/// # Arguments
/// * `ctx` - The instruction context containing validated accounts
///
/// # Returns
/// * `Ok(tvl)` - The calculated TVL in base units
/// * `Err(crate::ProtocolError::NoActiveVector)` - If no pricing vector is currently active
/// * `Err(crate::ProtocolError::Overflow)` - If mathematical overflow occurs during calculation
/// * `Err(crate::ProtocolError::InvalidVaultAccount)` - If vault account validation fails
///
/// # Events
/// * `GetTVLEvent` - Emitted with TVL, price, supply, and timestamp details
pub fn get_tvl(ctx: Context<GetTVL>) -> Result<u64> {
    let offer = ctx.accounts.offer.load()?;
    let current_time = Clock::get()?.unix_timestamp as u64;

    let (_, current_price) = get_active_vector_and_current_price(&offer, current_time)?;

    let vault_amount = read_optional_token_account_amount(
        &ctx.accounts.vault_token_out_account,
        &ctx.accounts.token_out_program,
    )?;
    let token_supply =
        calculate_circulating_supply(ctx.accounts.token_out_mint.supply, vault_amount)?;

    let tvl = calculate_shared_tvl(token_supply, current_price)
        .map_err(|_| error!(crate::ProtocolError::Overflow))?;

    msg!(
        "TVL Info - Offer PDA: {}, TVL: {}, Current Price: {}, Token Supply: {}, Timestamp: {}",
        ctx.accounts.offer.key(),
        tvl,
        current_price,
        token_supply,
        current_time
    );

    emit!(GetTVLEvent {
        offer_pda: ctx.accounts.offer.key(),
        tvl,
        current_price,
        token_supply,
        timestamp: current_time,
    });

    Ok(tvl)
}

pub fn get_tvl_v2(ctx: Context<GetTVLV2>) -> Result<u64> {
    let offer = ctx.accounts.offer.load()?;
    let current_time = Clock::get()?.unix_timestamp as u64;

    let (_, current_price) = get_active_vector_and_current_price(&offer, current_time)?;

    let excluded_amount = load_circulating_supply_excluded_balance_amount(
        ctx.program_id,
        &ctx.accounts
            .circulating_supply_excluded_balance
            .to_account_info(),
    )?;

    let token_supply =
        calculate_circulating_supply(ctx.accounts.token_out_mint.supply, excluded_amount)?;

    let tvl = calculate_shared_tvl(token_supply, current_price)
        .map_err(|_| error!(crate::ProtocolError::Overflow))?;

    msg!(
        "TVL Info - Offer PDA: {}, TVL: {}, Current Price: {}, Token Supply: {}, Timestamp: {}",
        ctx.accounts.offer.key(),
        tvl,
        current_price,
        token_supply,
        current_time
    );

    emit!(GetTVLEvent {
        offer_pda: ctx.accounts.offer.key(),
        tvl,
        current_price,
        token_supply,
        timestamp: current_time,
    });

    Ok(tvl)
}
