use crate::constants::seeds;
use crate::utils::load_optional_pda_account;
use anchor_lang::prelude::*;

/// Redemption offer for converting Rwa tokens back to a paired output asset.
///
/// Manages the redemption process where users can exchange Rwa (in-token)
/// for the original offer's input asset (out-token) at the current NAV price.
/// This is the inverse of the standard Offer which exchanges that asset for Rwa.
#[account]
#[derive(InitSpace)]
pub struct RedemptionOffer {
    /// Reference to the original Offer PDA that this redemption offer is associated with
    pub offer: Pubkey,
    /// Input token mint for redemptions (e.g., Rwa)
    pub token_in_mint: Pubkey,
    /// Output token mint for redemptions (e.g., USDC)
    pub token_out_mint: Pubkey,
    /// Cumulative total of all executed redemptions over the contract's lifetime
    ///
    /// This tracks the cumulative gross token_in amount fulfilled across redemption
    /// requests, before fee deduction. Net token_in may be burned or transferred
    /// to proceeds depending on mint authority.
    /// Uses u128 for aggregate accounting across requests.
    pub executed_redemptions: u128,
    /// Total amount of pending redemption requests
    ///
    /// This tracks Rwa tokens that are locked in pending redemption requests.
    /// Uses u128 for aggregate accounting across requests.
    pub requested_redemptions: u128,
    /// Fee in basis points (1000 = 10%) charged when the worker fulfills requests
    pub fee_basis_points: u16,
    /// Counter for sequential redemption request numbering
    /// Increments with each new redemption request created
    pub request_counter: u64,
    /// PDA bump seed for account derivation
    pub bump: u8,
    /// Target token-out balance for the redemption vault as basis points of TVL.
    ///
    /// A value of 0 disables automatic inflow into the redemption vault; net inflow
    /// goes to the configured proceeds vault instead.
    pub vault_target_bps: u16,
    /// Whether the redemption offer is disabled by targeted emergency controls (0 = false, 1 = true)
    disabled: u8,
    /// Fee in basis points (1000 = 10%) charged when Prop RFQ sell fulfills redemptions
    pub fee_basis_points_prop_rfq_sell: u16,
    /// Reserved space for future fields
    pub reserved: [u8; 104],
}

impl RedemptionOffer {
    pub fn is_disabled(&self) -> bool {
        self.disabled != 0
    }

    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = if disabled { 1 } else { 0 };
    }

    pub fn require_enabled(&self) -> Result<()> {
        require!(
            !self.is_disabled(),
            crate::ProtocolError::RedemptionOfferDisabled
        );
        Ok(())
    }
}

pub(crate) fn load_optional_checked_redemption_offer(
    program_id: &Pubkey,
    redemption_offer_account: &UncheckedAccount,
    offer_key: Pubkey,
    token_in_mint: Pubkey,
    token_out_mint: Pubkey,
) -> Result<Option<RedemptionOffer>> {
    let (expected_redemption_offer, _) = Pubkey::find_program_address(
        &[
            seeds::REDEMPTION_OFFER,
            token_in_mint.as_ref(),
            token_out_mint.as_ref(),
        ],
        program_id,
    );
    require_keys_eq!(
        redemption_offer_account.key(),
        expected_redemption_offer,
        crate::ProtocolError::InvalidRedemptionOffer
    );

    let Some(redemption_offer) = load_optional_pda_account::<RedemptionOffer>(
        &redemption_offer_account.to_account_info(),
        program_id,
        crate::ProtocolError::InvalidRedemptionOfferOwner.into(),
        crate::ProtocolError::InvalidRedemptionOfferData.into(),
    )?
    else {
        return Ok(None);
    };

    require_keys_eq!(
        redemption_offer.offer,
        offer_key,
        crate::ProtocolError::InvalidRedemptionOffer
    );
    require_keys_eq!(
        redemption_offer.token_in_mint,
        token_in_mint,
        crate::ProtocolError::InvalidRedemptionOffer
    );
    require_keys_eq!(
        redemption_offer.token_out_mint,
        token_out_mint,
        crate::ProtocolError::InvalidRedemptionOffer
    );

    Ok(Some(redemption_offer))
}

#[account]
#[derive(InitSpace)]
pub struct RedemptionRequest {
    /// Reference to the RedemptionOffer PDA
    pub offer: Pubkey,
    /// Unique sequential identifier for this request (counter value used for PDA derivation)
    pub request_id: u64,
    /// User requesting the redemption
    pub redeemer: Pubkey,
    /// Amount of token_in tokens requested for redemption
    pub amount: u64,
    /// PDA bump seed for account derivation
    pub bump: u8,
    /// Amount of token_in tokens that have already been fulfilled (partial fulfillment tracking)
    ///
    /// Starts at 0. Incremented by each partial or full fulfillment call.
    /// When fulfilled_amount == amount the request is fully settled and the account is closed.
    /// remaining = amount - fulfilled_amount is still locked in the redemption vault.
    pub fulfilled_amount: u64,
    /// Reserved space for future fields
    pub reserved: [u8; 119],
}
