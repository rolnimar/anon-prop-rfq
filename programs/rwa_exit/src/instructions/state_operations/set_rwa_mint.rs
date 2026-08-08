use crate::constants::seeds;
use crate::state::State;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

/// Event emitted when the Rwa token mint is successfully updated
///
/// Provides transparency for tracking Rwa mint configuration changes.
#[event]
pub struct RwaMintUpdatedEvent {
    /// The previous Rwa mint public key before the update
    pub old_rwa_mint: Pubkey,
    /// The new Rwa mint public key after the update
    pub new_rwa_mint: Pubkey,
}

/// Account structure for configuring the Rwa token mint
///
/// This struct defines the accounts required to set or update the Rwa token
/// mint address in the program state. Only the boss can configure this setting.
#[derive(Accounts)]
pub struct SetRwaMint<'info> {
    /// Program state account containing the Rwa mint configuration
    ///
    /// Must be mutable to allow Rwa mint updates and have the boss account
    /// as the authorized signer for mint configuration management.
    #[account(
        mut,
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = boss
    )]
    pub state: Account<'info, State>,

    /// The boss account authorized to configure the Rwa mint
    pub boss: Signer<'info>,

    /// The Rwa token mint account to be set in program state
    pub rwa_mint: InterfaceAccount<'info, Mint>,
}

/// Configures the Rwa token mint address in program state
///
/// This instruction allows the boss to set or update the Rwa token mint that
/// the program recognizes for operations. The Rwa mint is used for calculating
/// market metrics and token-related operations within the protocol.
///
/// # Arguments
/// * `ctx` - The instruction context containing validated accounts
///
/// # Returns
/// * `Ok(())` - If the Rwa mint is successfully configured
///
/// # Access Control
/// - Only the boss can call this instruction
/// - Boss account must match the one stored in program state
///
/// # Effects
/// - Updates the program state's rwa_mint field
/// - Configures which token mint is recognized as Rwa
/// - Affects future market calculations and operations
///
/// # Events
/// * `RwaMintUpdatedEvent` - Emitted with old and new Rwa mint addresses
pub fn set_rwa_mint(ctx: Context<SetRwaMint>) -> Result<()> {
    let state = &mut ctx.accounts.state;

    let old_rwa_mint = state.rwa_mint;
    state.rwa_mint = ctx.accounts.rwa_mint.key();

    msg!("Rwa mint updated: {}", state.rwa_mint);
    emit!(RwaMintUpdatedEvent {
        old_rwa_mint,
        new_rwa_mint: state.rwa_mint,
    });

    Ok(())
}
