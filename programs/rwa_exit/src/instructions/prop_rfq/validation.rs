use crate::constants::seeds;
use crate::state::State;
use anchor_lang::prelude::*;
use anchor_lang::{AnchorDeserialize, AnchorSerialize};

use super::config::PropRfqPairState;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapSide {
    Buy,
    Sell,
}

pub(crate) fn resolve_swap_side(
    state: &State,
    token_in_mint: Pubkey,
    token_out_mint: Pubkey,
) -> Result<(SwapSide, Pubkey)> {
    require!(
        token_in_mint != token_out_mint,
        crate::ProtocolError::InvalidSwapPair
    );

    if token_out_mint == state.rwa_mint && token_in_mint != state.rwa_mint {
        return Ok((SwapSide::Buy, token_in_mint));
    }

    if token_in_mint == state.rwa_mint && token_out_mint != state.rwa_mint {
        return Ok((SwapSide::Sell, token_out_mint));
    }

    Err(error!(crate::ProtocolError::InvalidSwapPair))
}

pub(crate) fn validate_canonical_offer(
    program_id: &Pubkey,
    state: &State,
    offer_key: Pubkey,
    token_in_mint: Pubkey,
    token_out_mint: Pubkey,
) -> Result<SwapSide> {
    let (side, asset_mint) = resolve_swap_side(state, token_in_mint, token_out_mint)?;
    let (expected_offer, _) = Pubkey::find_program_address(
        &[seeds::OFFER, asset_mint.as_ref(), state.rwa_mint.as_ref()],
        program_id,
    );
    require_keys_eq!(
        offer_key,
        expected_offer,
        crate::ProtocolError::OfferMismatch
    );
    Ok(side)
}

pub(crate) fn validate_prop_rfq_pair_state(
    state: &State,
    prop_rfq_pair_state: &PropRfqPairState,
    offer_key: Pubkey,
    token_in_mint: Pubkey,
    token_out_mint: Pubkey,
) -> Result<SwapSide> {
    let (side, asset_mint) = resolve_swap_side(state, token_in_mint, token_out_mint)?;
    require_keys_eq!(
        prop_rfq_pair_state.offer,
        offer_key,
        crate::ProtocolError::InvalidPropRfqPairState
    );
    require_keys_eq!(
        prop_rfq_pair_state.asset_mint,
        asset_mint,
        crate::ProtocolError::InvalidPropRfqPairState
    );
    require_keys_eq!(
        prop_rfq_pair_state.rwa_mint,
        state.rwa_mint,
        crate::ProtocolError::InvalidPropRfqPairState
    );
    require!(
        prop_rfq_pair_state.enabled,
        crate::ProtocolError::PropRfqPairDisabled
    );
    Ok(side)
}

pub(crate) fn validate_prop_rfq_pair_for_side(
    program_id: &Pubkey,
    state: &State,
    prop_rfq_pair_state: &PropRfqPairState,
    offer_key: Pubkey,
    token_in_mint: Pubkey,
    token_out_mint: Pubkey,
    expected_side: SwapSide,
) -> Result<()> {
    let side =
        validate_canonical_offer(program_id, state, offer_key, token_in_mint, token_out_mint)?;
    require!(side == expected_side, crate::ProtocolError::InvalidSwapPair);
    let pair_side = validate_prop_rfq_pair_state(
        state,
        prop_rfq_pair_state,
        offer_key,
        token_in_mint,
        token_out_mint,
    )?;
    require!(
        pair_side == expected_side,
        crate::ProtocolError::InvalidSwapPair
    );
    Ok(())
}
