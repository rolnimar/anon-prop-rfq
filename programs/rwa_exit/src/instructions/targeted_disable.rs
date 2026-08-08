use crate::state::State;
use anchor_lang::prelude::*;

pub(crate) fn authorize_targeted_disable(
    state: &State,
    signer: &Signer,
    disabled: bool,
) -> Result<()> {
    let boss_signed = state.boss == signer.key() && signer.is_signer;
    let admin_signed = state.admins.contains(&signer.key()) && signer.is_signer;

    if disabled {
        require!(
            boss_signed || admin_signed,
            crate::ProtocolError::UnauthorizedToDisableOffer
        );
    } else {
        require!(boss_signed, crate::ProtocolError::OnlyBossCanEnableOffer);
    }

    Ok(())
}
