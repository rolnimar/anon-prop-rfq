use crate::constants::seeds;
use crate::instructions::redemption::RedemptionOffer;
use crate::instructions::targeted_disable::authorize_targeted_disable;
use crate::state::State;
use anchor_lang::prelude::*;

#[event]
pub struct RedemptionOfferDisabledSetEvent {
    pub redemption_offer_pda: Pubkey,
    pub disabled: bool,
    pub signer: Pubkey,
}

#[derive(Accounts)]
pub struct SetRedemptionOfferDisabled<'info> {
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

    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
    )]
    pub state: Box<Account<'info, State>>,

    pub signer: Signer<'info>,
}

pub fn set_redemption_offer_disabled(
    ctx: Context<SetRedemptionOfferDisabled>,
    disabled: bool,
) -> Result<()> {
    authorize_targeted_disable(&ctx.accounts.state, &ctx.accounts.signer, disabled)?;

    ctx.accounts.redemption_offer.set_disabled(disabled);

    emit!(RedemptionOfferDisabledSetEvent {
        redemption_offer_pda: ctx.accounts.redemption_offer.key(),
        disabled,
        signer: ctx.accounts.signer.key(),
    });

    Ok(())
}
