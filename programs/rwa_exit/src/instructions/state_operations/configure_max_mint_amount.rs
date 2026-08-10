use crate::constants::seeds;
use crate::state::State;
use anchor_lang::prelude::*;

/// Event emitted when the per-mint amount cap is configured.
#[event]
pub struct MaxMintAmountConfiguredEvent {
    /// The previous per-mint cap (0 = no cap)
    pub old_max_mint_amount: u64,
    /// The new per-mint cap (0 = no cap)
    pub new_max_mint_amount: u64,
}

#[derive(Accounts)]
pub struct ConfigureMaxMintAmount<'info> {
    #[account(
        mut,
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = boss
    )]
    pub state: Account<'info, State>,

    pub boss: Signer<'info>,
}

pub fn configure_max_mint_amount(
    ctx: Context<ConfigureMaxMintAmount>,
    max_mint_amount: u64,
) -> Result<()> {
    let state = &mut ctx.accounts.state;

    let old_max_mint_amount = state.max_mint_amount;
    state.max_mint_amount = max_mint_amount;

    msg!(
        "Max mint per mint configured: {} (previous: {})",
        max_mint_amount,
        old_max_mint_amount
    );

    emit!(MaxMintAmountConfiguredEvent {
        old_max_mint_amount,
        new_max_mint_amount: max_mint_amount,
    });

    Ok(())
}
