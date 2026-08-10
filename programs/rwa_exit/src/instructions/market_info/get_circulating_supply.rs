use crate::constants::seeds;
use crate::instructions::market_info::{
    calculate_circulating_supply, load_circulating_supply_excluded_balance_amount,
};

use crate::state::State;
use crate::utils::token_utils::read_optional_token_account_amount;
use anchor_lang::prelude::*;
use anchor_lang::Accounts;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token_interface::{Mint, TokenInterface};

/// Event emitted when circulating supply calculation is completed
///
/// Provides transparency for tracking token supply distribution and vault holdings.
#[event]
pub struct GetCirculatingSupplyEvent {
    /// Calculated circulating supply (total_supply - excluded balances) in base units
    pub circulating_supply: u64,
    /// Total token supply from the mint account in base units
    pub total_supply: u64,
    /// Vault token amount excluded from circulation in base units
    pub vault_amount: u64,
    /// Unix timestamp when the calculation was performed
    pub timestamp: u64,
}

#[event]
pub struct GetCirculatingSupplyV2Event {
    pub circulating_supply: u64,
    pub total_supply: u64,
    pub excluded_amount: u64,
    pub timestamp: u64,
}

/// Account structure for querying circulating supply information
///
/// This struct defines the accounts required to calculate the circulating supply
/// of Rwa tokens by subtracting vault holdings from total supply.
/// All accounts are validated to ensure accurate calculation.
#[derive(Accounts)]
pub struct GetCirculatingSupply<'info> {
    /// The Rwa token mint containing total supply information
    pub rwa_mint: InterfaceAccount<'info, Mint>,

    /// Program state account containing the Rwa mint reference
    #[account(seeds = [seeds::STATE], bump = state.bump, has_one = rwa_mint)]
    pub state: Box<Account<'info, State>>,

    /// CHECK: PDA derivation is validated by seeds constraint
    #[account(seeds = [seeds::OFFER_VAULT_AUTHORITY], bump)]
    pub vault_authority: UncheckedAccount<'info>,

    /// CHECK: Account address is validated by the constraint below to allow passing uninitialized vault account
    #[account(
        constraint = rwa_vault_account.key()
            == get_associated_token_address_with_program_id(
                &vault_authority.key(),
                &state.rwa_mint.key(),
                &token_program.key(),
            ) @ crate::ProtocolError::InvalidVaultAccount
    )]
    pub rwa_vault_account: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct GetCirculatingSupplyV2<'info> {
    pub rwa_mint: InterfaceAccount<'info, Mint>,

    #[account(seeds = [seeds::STATE], bump = state.bump, has_one = rwa_mint)]
    pub state: Box<Account<'info, State>>,

    /// CHECK: PDA derivation is validated by seeds constraint; uninitialized data means zero.
    #[account(seeds = [seeds::CIRCULATING_SUPPLY_EXCLUDED_BALANCE], bump)]
    pub circulating_supply_excluded_balance: UncheckedAccount<'info>,
}

/// Calculates and returns the current circulating supply of Rwa tokens
///
/// This read-only instruction calculates the circulating supply by subtracting
/// excluded balances from the total token supply. The excluded balance is the
/// offer-vault Rwa holding.
///
/// Formula: `circulating_supply = total_supply - vault_amount`
///
/// The vault Rwa account can be uninitialized (treated as zero balance) or
/// contain tokens that should be excluded from circulation calculations.
///
/// # Arguments
/// * `ctx` - The instruction context containing validated accounts
///
/// # Returns
/// * `Ok(circulating_supply)` - The calculated circulating supply in base units
/// * `Err(crate::ProtocolError::InvalidVaultAccount)` - If vault account validation fails
///
/// # Events
/// * `GetCirculatingSupplyEvent` - Emitted with calculation details and timestamp
pub fn get_circulating_supply(ctx: Context<GetCirculatingSupply>) -> Result<u64> {
    let current_time = Clock::get()?.unix_timestamp as u64;

    let vault_amount = read_optional_token_account_amount(
        &ctx.accounts.rwa_vault_account,
        &ctx.accounts.token_program,
    )?;
    let total_supply = ctx.accounts.rwa_mint.supply;
    let circulating_supply = calculate_circulating_supply(total_supply, vault_amount)?;

    msg!(
        "Circulating Supply Info - Circulating Supply: {}, Total Supply: {}, Vault Amount: {}, Timestamp: {}",
        circulating_supply,
        total_supply,
        vault_amount,
        current_time
    );

    emit!(GetCirculatingSupplyEvent {
        circulating_supply,
        total_supply,
        vault_amount,
        timestamp: current_time,
    });

    Ok(circulating_supply)
}

pub fn get_circulating_supply_v2(ctx: Context<GetCirculatingSupplyV2>) -> Result<u64> {
    let current_time = Clock::get()?.unix_timestamp as u64;

    let excluded_amount = load_circulating_supply_excluded_balance_amount(
        ctx.program_id,
        &ctx.accounts
            .circulating_supply_excluded_balance
            .to_account_info(),
    )?;

    let total_supply = ctx.accounts.rwa_mint.supply;

    let circulating_supply = calculate_circulating_supply(total_supply, excluded_amount)?;

    msg!(
        "Circulating Supply Info - Circulating Supply: {}, Total Supply: {}, Excluded Amount: {}, Timestamp: {}",
        circulating_supply,
        total_supply,
        excluded_amount,
        current_time
    );

    emit!(GetCirculatingSupplyV2Event {
        circulating_supply,
        total_supply,
        excluded_amount,
        timestamp: current_time,
    });

    Ok(circulating_supply)
}
