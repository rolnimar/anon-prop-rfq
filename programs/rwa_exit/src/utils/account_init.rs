use anchor_lang::{prelude::*, solana_program::program::invoke_signed, system_program};
use solana_system_interface::instruction::create_account_allow_prefund;

pub trait PdaAccountInit: AccountSerialize + AccountDeserialize {
    fn pda_seed_prefixes() -> &'static [&'static [u8]];
    fn init_space() -> usize;
    fn init_value(bump: u8) -> Self;
    fn invalid_owner_error() -> Error;
    fn invalid_data_error() -> Error;
}

/// Loads a program-owned PDA or initializes a system-owned, zero-data PDA.
///
/// Initialization uses SIMD-0312 so an attacker cannot block creation by
/// prefunding the deterministic address. The runtime must have
/// `CreateAccountAllowPrefund` enabled.
pub fn load_or_init_pda_account<'info, T>(
    account: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    _system_program_account: &AccountInfo<'info>,
    program_id: &Pubkey,
    bump: u8,
) -> Result<T>
where
    T: PdaAccountInit,
{
    let bump_seed = [bump];
    let mut signer_seed_parts: Vec<&[u8]> = T::pda_seed_prefixes().to_vec();
    signer_seed_parts.push(&bump_seed);
    let signer_seeds = [&signer_seed_parts[..]];

    if account.owner == &system_program::ID {
        if !account.data_is_empty() {
            return Err(T::invalid_owner_error());
        }

        let rent_lamports = Rent::get()?.minimum_balance(T::init_space());
        let lamports_to_transfer = rent_lamports.saturating_sub(account.lamports());
        let payer_and_lamports =
            (lamports_to_transfer > 0).then_some((payer.key, lamports_to_transfer));
        let create_account_ix = create_account_allow_prefund(
            account.key,
            payer_and_lamports,
            T::init_space() as u64,
            program_id,
        );
        if payer_and_lamports.is_some() {
            invoke_signed(
                &create_account_ix,
                &[account.clone(), payer.clone()],
                &signer_seeds,
            )?;
        } else {
            invoke_signed(
                &create_account_ix,
                std::slice::from_ref(account),
                &signer_seeds,
            )?;
        }

        return Ok(T::init_value(bump));
    }

    if account.owner != program_id {
        return Err(T::invalid_owner_error());
    }
    deserialize_pda_account(account, T::invalid_data_error())
}

pub fn load_pda_account<T>(
    account: &AccountInfo,
    program_id: &Pubkey,
    invalid_owner_error: Error,
    invalid_data_error: Error,
) -> Result<T>
where
    T: AccountDeserialize,
{
    if account.owner != program_id {
        return Err(invalid_owner_error);
    }
    deserialize_pda_account(account, invalid_data_error)
}

pub fn load_optional_pda_account<T>(
    account: &AccountInfo,
    program_id: &Pubkey,
    invalid_owner_error: Error,
    invalid_data_error: Error,
) -> Result<Option<T>>
where
    T: AccountDeserialize,
{
    if account.owner == &system_program::ID {
        if account.data_is_empty() {
            return Ok(None);
        }
        return Err(invalid_data_error);
    }

    load_pda_account(account, program_id, invalid_owner_error, invalid_data_error).map(Some)
}

pub fn store_pda_account<T>(account: &AccountInfo, value: &T) -> Result<()>
where
    T: AccountSerialize,
{
    let mut data = account.try_borrow_mut_data()?;
    let dst: &mut [u8] = &mut data;
    let mut cursor = std::io::Cursor::new(dst);
    value.try_serialize(&mut cursor)
}

fn deserialize_pda_account<T>(account: &AccountInfo, invalid_data_error: Error) -> Result<T>
where
    T: AccountDeserialize,
{
    let data = account.try_borrow_data()?;
    let mut slice: &[u8] = &data;
    T::try_deserialize(&mut slice).map_err(|_| invalid_data_error)
}
