use crate::constants::{seeds, MAX_ALLOWED_FEE_BPS};
use crate::instructions::Offer;
use crate::state::State;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

/// Event emitted when an offer's permissionless fee is updated.
#[event]
pub struct OfferPermissionlessFeeUpdatedEvent {
    /// The PDA address of the offer whose permissionless fee was updated.
    pub offer_pda: Pubkey,
    /// Previous permissionless fee in basis points (1000 = 10%).
    pub old_fee_basis_points_permissionless: u16,
    /// New permissionless fee in basis points (1000 = 10%).
    pub new_fee_basis_points_permissionless: u16,
    /// The boss account that authorized the fee update.
    pub boss: Pubkey,
}

/// Account structure for updating an offer's permissionless fee configuration.
#[derive(Accounts)]
pub struct UpdateOfferPermissionlessFee<'info> {
    /// The offer account whose permissionless fee will be updated.
    #[account(
        mut,
        seeds = [
            seeds::OFFER,
            token_in_mint.key().as_ref(),
            token_out_mint.key().as_ref()
        ],
        bump = offer.load()?.bump
    )]
    pub offer: AccountLoader<'info, Offer>,

    /// The input token mint account for offer validation.
    #[account(
        constraint =
            token_in_mint.key() == offer.load()?.token_in_mint
            @ crate::ProtocolError::InvalidTokenInMint
    )]
    pub token_in_mint: InterfaceAccount<'info, Mint>,

    /// The output token mint account for offer validation.
    #[account(
        constraint =
            token_out_mint.key() == offer.load()?.token_out_mint
            @ crate::ProtocolError::InvalidTokenOutMint
    )]
    pub token_out_mint: InterfaceAccount<'info, Mint>,

    /// Program state account containing boss authorization.
    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = boss
    )]
    pub state: Account<'info, State>,

    /// The boss account authorized to update offer fees.
    pub boss: Signer<'info>,
}

/// Updates the fee charged only by permissionless offer execution.
///
/// Regular offer execution continues to use `Offer::fee_basis_points`.
pub fn update_offer_permissionless_fee(
    ctx: Context<UpdateOfferPermissionlessFee>,
    new_fee_basis_points_permissionless: u16,
) -> Result<()> {
    require!(
        new_fee_basis_points_permissionless <= MAX_ALLOWED_FEE_BPS,
        crate::ProtocolError::InvalidFee
    );

    let offer = &mut ctx.accounts.offer.load_mut()?;
    let old_fee_basis_points_permissionless = offer.fee_basis_points_permissionless;
    offer.set_permissionless_fee_basis_points(new_fee_basis_points_permissionless);

    msg!(
        "Offer permissionless fee updated for offer: {}, old fee: {}, new fee: {}",
        ctx.accounts.offer.key(),
        old_fee_basis_points_permissionless,
        new_fee_basis_points_permissionless
    );

    emit!(OfferPermissionlessFeeUpdatedEvent {
        offer_pda: ctx.accounts.offer.key(),
        old_fee_basis_points_permissionless,
        new_fee_basis_points_permissionless,
        boss: ctx.accounts.boss.key(),
    });

    Ok(())
}
