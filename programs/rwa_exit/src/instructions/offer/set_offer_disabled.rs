use crate::constants::seeds;
use crate::instructions::targeted_disable::authorize_targeted_disable;
use crate::instructions::Offer;
use crate::state::State;
use anchor_lang::prelude::*;

#[event]
pub struct OfferDisabledSetEvent {
    pub offer_pda: Pubkey,
    pub disabled: bool,
    pub signer: Pubkey,
}

#[derive(Accounts)]
pub struct SetOfferDisabled<'info> {
    #[account(mut)]
    pub offer: AccountLoader<'info, Offer>,

    #[account(
        seeds = [seeds::STATE],
        bump = state.bump,
    )]
    pub state: Box<Account<'info, State>>,

    pub signer: Signer<'info>,
}

pub fn set_offer_disabled(ctx: Context<SetOfferDisabled>, disabled: bool) -> Result<()> {
    authorize_targeted_disable(&ctx.accounts.state, &ctx.accounts.signer, disabled)?;

    let mut offer = ctx.accounts.offer.load_mut()?;
    offer.set_disabled(disabled);

    emit!(OfferDisabledSetEvent {
        offer_pda: ctx.accounts.offer.key(),
        disabled,
        signer: ctx.accounts.signer.key(),
    });

    Ok(())
}
