use crate::constants::seeds;
use crate::instructions::offer::process_offer_core;
use crate::instructions::redemption::process_redemption_core;
use crate::instructions::Offer;
use crate::state::State;
use crate::utils::has_transfer_fee;
use anchor_lang::solana_program::program::set_return_data;
use anchor_lang::{prelude::*, Accounts, AnchorDeserialize, AnchorSerialize};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use super::config::PropRfqPairState;
use super::pricing::{
    apply_hard_wall_liquidity_factor, sell_quote_liquidity_context, validate_market_stats_pda,
};
use super::validation::{validate_prop_rfq_pair_for_side, SwapSide};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct SwapQuote {
    pub offer: Pubkey,
    pub token_in_mint: Pubkey,
    pub token_out_mint: Pubkey,
    pub token_in_amount: u64,
    pub token_in_net_amount: u64,
    pub token_in_fee_amount: u64,
    pub token_out_amount: u64,
    pub minimum_out: u64,
    pub current_price: u64,
    pub quoted_at: i64,
}

struct SwapQuoteComputation {
    current_price: u64,
    token_in_net_amount: u64,
    token_in_fee_amount: u64,
    token_out_amount: u64,
}

#[derive(Accounts)]
pub struct QuoteSwapBuy<'info> {
    pub offer: AccountLoader<'info, Offer>,

    #[account(
        seeds = [seeds::PROP_RFQ_PAIR_STATE, offer.key().as_ref()],
        bump = prop_rfq_pair_state.bump
    )]
    pub prop_rfq_pair_state: Box<Account<'info, PropRfqPairState>>,

    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        constraint = !state.is_killed @ crate::ProtocolError::KillSwitchActivated
    )]
    pub state: Box<Account<'info, State>>,

    pub token_in_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_out_mint: Box<InterfaceAccount<'info, Mint>>,
}

#[derive(Accounts)]
pub struct QuoteSwapSell<'info> {
    pub offer: AccountLoader<'info, Offer>,

    #[account(
        seeds = [seeds::PROP_RFQ_PAIR_STATE, offer.key().as_ref()],
        bump = prop_rfq_pair_state.bump
    )]
    pub prop_rfq_pair_state: Box<Account<'info, PropRfqPairState>>,

    #[account(
        seeds = [
            seeds::REDEMPTION_OFFER,
            token_in_mint.key().as_ref(),
            token_out_mint.key().as_ref()
        ],
        bump
    )]
    /// CHECK: PDA address is validated by seeds; data is optional and loaded in instruction logic.
    pub redemption_offer: UncheckedAccount<'info>,

    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        constraint = !state.is_killed @ crate::ProtocolError::KillSwitchActivated
    )]
    pub state: Box<Account<'info, State>>,

    /// CHECK: PDA derivation validated by seeds constraint
    #[account(seeds = [seeds::REDEMPTION_OFFER_VAULT_AUTHORITY], bump)]
    pub redemption_vault_authority: UncheckedAccount<'info>,

    #[account(
        associated_token::mint = token_out_mint,
        associated_token::authority = redemption_vault_authority,
        associated_token::token_program = token_out_program
    )]
    pub redemption_vault_token_out_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_in_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_out_mint: Box<InterfaceAccount<'info, Mint>>,
    pub token_out_program: Interface<'info, TokenInterface>,

    /// CHECK: PDA address is validated in instruction logic; data is loaded only if initialized.
    pub market_stats: UncheckedAccount<'info>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_swap_buy_quote(
    program_id: &Pubkey,
    state: &State,
    offer_key: Pubkey,
    offer: &Offer,
    prop_rfq_pair_state: &PropRfqPairState,
    token_in_amount: u64,
    token_in_mint: &InterfaceAccount<Mint>,
    token_out_mint: &InterfaceAccount<Mint>,
) -> Result<SwapQuote> {
    let quoted_at = Clock::get()?.unix_timestamp;
    offer.require_enabled()?;

    validate_prop_rfq_pair_for_side(
        program_id,
        state,
        prop_rfq_pair_state,
        offer_key,
        token_in_mint.key(),
        token_out_mint.key(),
        SwapSide::Buy,
    )?;

    let result = process_offer_core(
        offer,
        token_in_amount,
        token_in_mint,
        token_out_mint,
        offer.permissionless_fee_basis_points(),
    )
    .map(|result| SwapQuoteComputation {
        current_price: result.current_price,
        token_in_net_amount: result.token_in_net_amount,
        token_in_fee_amount: result.token_in_fee_amount,
        token_out_amount: result.token_out_amount,
    })?;

    Ok(SwapQuote {
        offer: offer_key,
        token_in_mint: token_in_mint.key(),
        token_out_mint: token_out_mint.key(),
        token_in_amount,
        token_in_net_amount: result.token_in_net_amount,
        token_in_fee_amount: result.token_in_fee_amount,
        token_out_amount: result.token_out_amount,
        minimum_out: result.token_out_amount,
        current_price: result.current_price,
        quoted_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_swap_sell_quote(
    program_id: &Pubkey,
    state: &State,
    offer_key: Pubkey,
    offer: &Offer,
    prop_rfq_pair_state: &PropRfqPairState,
    actual_liquidity: u64,
    hard_wall_reserve: u64,
    redemption_fee_basis_points: u16,
    token_in_amount: u64,
    token_in_mint: &InterfaceAccount<Mint>,
    token_out_mint: &InterfaceAccount<Mint>,
) -> Result<SwapQuote> {
    let quoted_at = Clock::get()?.unix_timestamp;
    offer.require_enabled()?;

    validate_prop_rfq_pair_for_side(
        program_id,
        state,
        prop_rfq_pair_state,
        offer_key,
        token_in_mint.key(),
        token_out_mint.key(),
        SwapSide::Sell,
    )?;

    let mut result = process_redemption_core(
        offer,
        token_in_amount,
        token_in_mint,
        token_out_mint,
        redemption_fee_basis_points,
        prop_rfq_pair_state.minimum_sell_haircut_rwa,
    )
    .map(|result| SwapQuoteComputation {
        current_price: result.price,
        token_in_net_amount: result.token_in_net_amount,
        token_in_fee_amount: result.token_in_fee_amount,
        token_out_amount: result.token_out_amount,
    })?;
    result.token_out_amount = apply_hard_wall_liquidity_factor(
        result.token_out_amount,
        actual_liquidity,
        hard_wall_reserve,
        prop_rfq_pair_state,
    )?;

    Ok(SwapQuote {
        offer: offer_key,
        token_in_mint: token_in_mint.key(),
        token_out_mint: token_out_mint.key(),
        token_in_amount,
        token_in_net_amount: result.token_in_net_amount,
        token_in_fee_amount: result.token_in_fee_amount,
        token_out_amount: result.token_out_amount,
        minimum_out: result.token_out_amount,
        current_price: result.current_price,
        quoted_at,
    })
}

pub fn quote_swap_buy(ctx: Context<QuoteSwapBuy>, token_in_amount: u64) -> Result<()> {
    let offer = ctx.accounts.offer.load()?;
    require_quote_mints_without_transfer_fees(
        &ctx.accounts.token_in_mint,
        &ctx.accounts.token_out_mint,
    )?;
    let quote = build_swap_buy_quote(
        ctx.program_id,
        &ctx.accounts.state,
        ctx.accounts.offer.key(),
        &offer,
        &ctx.accounts.prop_rfq_pair_state,
        token_in_amount,
        &ctx.accounts.token_in_mint,
        &ctx.accounts.token_out_mint,
    )?;
    set_swap_quote_return_data(&quote)?;

    msg!(
        "Swap buy quote - offer: {}, token_in: {}, minimum_out: {}",
        quote.offer,
        quote.token_in_amount,
        quote.minimum_out
    );

    Ok(())
}

pub fn quote_swap_sell(ctx: Context<QuoteSwapSell>, token_in_amount: u64) -> Result<()> {
    let offer = ctx.accounts.offer.load()?;
    require_quote_mints_without_transfer_fees(
        &ctx.accounts.token_in_mint,
        &ctx.accounts.token_out_mint,
    )?;
    validate_market_stats_pda(ctx.program_id, &ctx.accounts.market_stats)?;
    let liquidity = sell_quote_liquidity_context(
        ctx.program_id,
        &ctx.accounts.redemption_offer,
        ctx.accounts.offer.key(),
        ctx.accounts.token_in_mint.key(),
        ctx.accounts.token_out_mint.key(),
        &ctx.accounts.market_stats,
        ctx.accounts.redemption_vault_token_out_account.amount,
        ctx.accounts.token_out_mint.decimals,
        ctx.accounts.token_in_mint.decimals,
    )?;
    let quote = build_swap_sell_quote(
        ctx.program_id,
        &ctx.accounts.state,
        ctx.accounts.offer.key(),
        &offer,
        &ctx.accounts.prop_rfq_pair_state,
        liquidity.actual_liquidity,
        liquidity.hard_wall_reserve,
        liquidity.fee_basis_points_prop_rfq_sell,
        token_in_amount,
        &ctx.accounts.token_in_mint,
        &ctx.accounts.token_out_mint,
    )?;
    set_swap_quote_return_data(&quote)?;

    msg!(
        "Swap sell quote - offer: {}, redemption_offer: {}, token_in: {}, minimum_out: {}",
        quote.offer,
        ctx.accounts.redemption_offer.key(),
        quote.token_in_amount,
        quote.minimum_out
    );

    Ok(())
}

fn require_quote_mints_without_transfer_fees(
    token_in_mint: &InterfaceAccount<Mint>,
    token_out_mint: &InterfaceAccount<Mint>,
) -> Result<()> {
    require!(
        !has_transfer_fee(token_in_mint)?,
        crate::ProtocolError::TransferFeeNotSupported
    );
    require!(
        !has_transfer_fee(token_out_mint)?,
        crate::ProtocolError::TransferFeeNotSupported
    );
    Ok(())
}

fn set_swap_quote_return_data(quote: &SwapQuote) -> Result<()> {
    let mut serialized_quote = Vec::new();
    quote.serialize(&mut serialized_quote)?;
    set_return_data(&serialized_quote);
    Ok(())
}
