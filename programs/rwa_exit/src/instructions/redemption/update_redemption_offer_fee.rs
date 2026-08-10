use crate::constants::{seeds, MAX_ALLOWED_FEE_BPS, MAX_BASIS_POINTS};
use crate::instructions::redemption::RedemptionOffer;
use crate::state::State;
use anchor_lang::prelude::*;

/// Event emitted when a redemption offer's fee is successfully updated
///
/// Provides transparency for tracking fee changes and redemption offer configuration modifications.
#[event]
pub struct RedemptionOfferFeeUpdatedEvent {
    /// The PDA address of the redemption offer whose fee was updated
    pub redemption_offer_pda: Pubkey,
    /// Previous fee in basis points.
    pub old_fee_basis_points: u16,
    /// New fee in basis points, capped at 1000 bps (10%).
    pub new_fee_basis_points: u16,
    /// The boss account that authorized the fee update
    pub boss: Pubkey,
}

#[event]
pub struct RedemptionOfferPropRfqSellFeeUpdatedEvent {
    pub redemption_offer_pda: Pubkey,
    pub old_fee_basis_points_prop_rfq_sell: u16,
    pub new_fee_basis_points_prop_rfq_sell: u16,
    pub boss: Pubkey,
}

#[event]
pub struct RedemptionOfferVaultTargetUpdatedEvent {
    pub redemption_offer_pda: Pubkey,
    pub old_vault_target_bps: u16,
    pub new_vault_target_bps: u16,
    pub boss: Pubkey,
}

/// Account structure for updating a redemption offer's fee configuration
///
/// This struct defines the accounts required to modify a redemption offer's fee
/// or vault target. Only the boss can update these redemption offer settings.
#[derive(Accounts)]
pub struct UpdateRedemptionOfferFee<'info> {
    /// The redemption offer account whose configuration will be updated.
    #[account(
        mut,
        seeds = [
            seeds::REDEMPTION_OFFER,
            redemption_offer.token_in_mint.as_ref(),
            redemption_offer.token_out_mint.as_ref()
        ],
        bump = redemption_offer.bump
    )]
    pub redemption_offer: Account<'info, RedemptionOffer>,

    /// Program state account containing boss authorization
    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = boss @ crate::ProtocolError::Unauthorized
    )]
    pub state: Account<'info, State>,

    /// The boss account authorized to update redemption offer fees
    pub boss: Signer<'info>,
}

/// Updates the fee configuration for an existing redemption offer
///
/// This instruction allows the boss to modify the fee charged when fulfilling
/// redemption requests for a specific redemption offer. The fee applies to all future
/// redemption fulfillments and is deducted from the token_in amount before calculating
/// token_out exchange amounts.
///
/// # Arguments
/// * `ctx` - The instruction context containing validated accounts
/// * `new_fee_basis_points` - New fee in basis points (1000 = 10%, 500 = 5%)
///
/// # Returns
/// * `Ok(())` - If the fee is successfully updated
/// * `Err(crate::ProtocolError::InvalidFee)` - If fee exceeds 1000 basis points (10%)
/// * `Err(crate::ProtocolError::Unauthorized)` - If caller is not the boss
///
/// # Access Control
/// - Only the boss can call this instruction
/// - Boss account must match the one stored in program state
///
/// # Effects
/// - Updates the redemption offer's fee_basis_points field
/// - Affects all future redemption fulfillments for this offer
/// - Does not modify existing redemption requests
///
/// # Events
/// * `RedemptionOfferFeeUpdatedEvent` - Emitted with old and new fee values
pub fn update_redemption_offer_fee(
    ctx: Context<UpdateRedemptionOfferFee>,
    new_fee_basis_points: u16,
) -> Result<()> {
    // Validate fee is within valid range (0-1000 basis points = 0-10%)
    require!(
        new_fee_basis_points <= MAX_ALLOWED_FEE_BPS,
        crate::ProtocolError::InvalidFee
    );

    let redemption_offer = &mut ctx.accounts.redemption_offer;

    // Validate this is not a no-op (setting the same fee)
    require!(
        new_fee_basis_points != redemption_offer.fee_basis_points,
        crate::ProtocolError::NoChange
    );

    // Store old fee for event
    let old_fee_basis_points = redemption_offer.fee_basis_points;

    // Update the fee
    redemption_offer.fee_basis_points = new_fee_basis_points;

    msg!(
        "Redemption offer fee updated for offer: {}, old fee: {}, new fee: {}",
        ctx.accounts.redemption_offer.key(),
        old_fee_basis_points,
        new_fee_basis_points
    );

    emit!(RedemptionOfferFeeUpdatedEvent {
        redemption_offer_pda: ctx.accounts.redemption_offer.key(),
        old_fee_basis_points,
        new_fee_basis_points,
        boss: ctx.accounts.boss.key(),
    });

    Ok(())
}

pub fn update_redemption_offer_prop_rfq_sell_fee(
    ctx: Context<UpdateRedemptionOfferFee>,
    new_fee_basis_points_prop_rfq_sell: u16,
) -> Result<()> {
    require!(
        new_fee_basis_points_prop_rfq_sell <= MAX_ALLOWED_FEE_BPS,
        crate::ProtocolError::InvalidFee
    );

    let redemption_offer = &mut ctx.accounts.redemption_offer;
    require!(
        new_fee_basis_points_prop_rfq_sell != redemption_offer.fee_basis_points_prop_rfq_sell,
        crate::ProtocolError::NoChange
    );

    let old_fee_basis_points_prop_rfq_sell = redemption_offer.fee_basis_points_prop_rfq_sell;
    redemption_offer.fee_basis_points_prop_rfq_sell = new_fee_basis_points_prop_rfq_sell;

    emit!(RedemptionOfferPropRfqSellFeeUpdatedEvent {
        redemption_offer_pda: ctx.accounts.redemption_offer.key(),
        old_fee_basis_points_prop_rfq_sell,
        new_fee_basis_points_prop_rfq_sell,
        boss: ctx.accounts.boss.key(),
    });

    Ok(())
}

pub fn update_redemption_offer_vault_target(
    ctx: Context<UpdateRedemptionOfferFee>,
    new_vault_target_bps: u16,
) -> Result<()> {
    require!(
        new_vault_target_bps <= MAX_BASIS_POINTS,
        crate::ProtocolError::InvalidAmount
    );

    let redemption_offer = &mut ctx.accounts.redemption_offer;
    require!(
        new_vault_target_bps != redemption_offer.vault_target_bps,
        crate::ProtocolError::NoChange
    );

    let old_vault_target_bps = redemption_offer.vault_target_bps;
    redemption_offer.vault_target_bps = new_vault_target_bps;

    emit!(RedemptionOfferVaultTargetUpdatedEvent {
        redemption_offer_pda: ctx.accounts.redemption_offer.key(),
        old_vault_target_bps,
        new_vault_target_bps,
        boss: ctx.accounts.boss.key(),
    });

    Ok(())
}
