use crate::constants::seeds;
use crate::state::State;
use anchor_lang::prelude::*;

/// Event emitted when the protocol worker is updated.
#[event]
pub struct WorkerUpdatedEvent {
    /// Previous worker public key.
    pub old_worker: Pubkey,
    /// New worker public key.
    pub new_worker: Pubkey,
}

/// Accounts for boss-controlled worker configuration.
#[derive(Accounts)]
pub struct SetWorker<'info> {
    /// Global state containing the worker configuration.
    #[account(
        mut,
        seeds = [seeds::STATE],
        bump = state.bump,
        has_one = boss
    )]
    pub state: Account<'info, State>,

    /// Boss authorized to configure the worker.
    pub boss: Signer<'info>,
}

/// Sets the worker authorized to fulfill or cancel redemptions and settle BUFFER.
pub fn set_worker(ctx: Context<SetWorker>, new_worker: Pubkey) -> Result<()> {
    let state = &mut ctx.accounts.state;

    require!(new_worker != state.worker, crate::ProtocolError::NoChange);

    let old_worker = state.worker;
    state.worker = new_worker;

    msg!("Worker updated: {}", state.worker);
    emit!(WorkerUpdatedEvent {
        old_worker,
        new_worker: state.worker,
    });

    Ok(())
}
