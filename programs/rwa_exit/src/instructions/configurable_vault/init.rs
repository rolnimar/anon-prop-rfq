use crate::constants::seeds;
use crate::state::{ConfigurableVault, ConfigurableVaultKind};
use crate::utils::PdaAccountInit;
use anchor_lang::prelude::*;

pub(crate) struct ConfigurableVaultInit<const KIND: u8> {
    bump: u8,
}

impl<const KIND: u8> ConfigurableVaultInit<KIND> {
    fn kind() -> ConfigurableVaultKind {
        match KIND {
            0 => ConfigurableVaultKind::OfferFee,
            1 => ConfigurableVaultKind::ManagementFee,
            2 => ConfigurableVaultKind::PerformanceFee,
            3 => ConfigurableVaultKind::PropRfqBuyFee,
            4 => ConfigurableVaultKind::OfferProceeds,
            5 => ConfigurableVaultKind::PropRfqProceeds,
            6 => ConfigurableVaultKind::PermissionlessOfferFee,
            7 => ConfigurableVaultKind::RedemptionFee,
            8 => ConfigurableVaultKind::PropRfqSellFee,
            _ => unreachable!("invalid configurable vault kind"),
        }
    }
}

impl<const KIND: u8> PdaAccountInit for ConfigurableVaultInit<KIND> {
    fn pda_seed_prefixes() -> &'static [&'static [u8]] {
        match Self::kind() {
            ConfigurableVaultKind::OfferFee => &[seeds::CONFIGURABLE_VAULT, seeds::OFFER_FEE_VAULT],
            ConfigurableVaultKind::ManagementFee => {
                &[seeds::CONFIGURABLE_VAULT, seeds::MANAGEMENT_FEE_VAULT]
            }
            ConfigurableVaultKind::PerformanceFee => {
                &[seeds::CONFIGURABLE_VAULT, seeds::PERFORMANCE_FEE_VAULT]
            }
            ConfigurableVaultKind::PropRfqBuyFee => {
                &[seeds::CONFIGURABLE_VAULT, seeds::PROP_RFQ_BUY_FEE_VAULT]
            }
            ConfigurableVaultKind::OfferProceeds => {
                &[seeds::CONFIGURABLE_VAULT, seeds::OFFER_PROCEEDS_VAULT]
            }
            ConfigurableVaultKind::PropRfqProceeds => {
                &[seeds::CONFIGURABLE_VAULT, seeds::PROP_RFQ_PROCEEDS_VAULT]
            }
            ConfigurableVaultKind::PermissionlessOfferFee => &[
                seeds::CONFIGURABLE_VAULT,
                seeds::PERMISSIONLESS_OFFER_FEE_VAULT,
            ],
            ConfigurableVaultKind::RedemptionFee => {
                &[seeds::CONFIGURABLE_VAULT, seeds::REDEMPTION_FEE_VAULT]
            }
            ConfigurableVaultKind::PropRfqSellFee => {
                &[seeds::CONFIGURABLE_VAULT, seeds::PROP_RFQ_SELL_FEE_VAULT]
            }
        }
    }

    fn init_space() -> usize {
        8 + ConfigurableVault::INIT_SPACE
    }

    fn init_value(bump: u8) -> Self {
        Self { bump }
    }

    fn invalid_owner_error() -> Error {
        error!(crate::ProtocolError::InvalidConfigurableVaultOwner)
    }

    fn invalid_data_error() -> Error {
        error!(crate::ProtocolError::InvalidConfigurableVaultData)
    }
}

impl<const KIND: u8> AccountSerialize for ConfigurableVaultInit<KIND> {
    fn try_serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_all(ConfigurableVault::DISCRIMINATOR)?;
        AnchorSerialize::serialize(
            &ConfigurableVault {
                kind: Self::kind().as_u8(),
                withdrawal_destination: Pubkey::default(),
                bump: self.bump,
                reserved: [0; 31],
            },
            writer,
        )?;
        Ok(())
    }
}

impl<const KIND: u8> AccountDeserialize for ConfigurableVaultInit<KIND> {
    fn try_deserialize(buf: &mut &[u8]) -> Result<Self> {
        // Program-owned zeroed vaults are initialized before this path. Every
        // existing vault reaching it must carry the canonical discriminator.
        let vault = ConfigurableVault::try_deserialize(buf)?;
        require!(
            vault.kind == Self::kind().as_u8(),
            crate::ProtocolError::InvalidConfigurableVaultKind
        );
        Ok(Self { bump: vault.bump })
    }

    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::try_deserialize(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_valid_vault_body_with_invalid_discriminator() {
        let vault = ConfigurableVault {
            kind: ConfigurableVaultKind::OfferFee.as_u8(),
            withdrawal_destination: Pubkey::default(),
            bump: 254,
            reserved: [0; 31],
        };
        let mut data = Vec::new();
        vault.try_serialize(&mut data).unwrap();
        data[0] ^= u8::MAX;

        let result = ConfigurableVaultInit::<0>::try_deserialize(&mut data.as_slice());

        assert!(result.is_err());
    }
}
