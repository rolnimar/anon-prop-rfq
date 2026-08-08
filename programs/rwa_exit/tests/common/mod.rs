#![allow(dead_code)]
#![allow(unused_imports)]

use anchor_lang::AccountDeserialize;
use litesvm::LiteSVM;
use rwa_exit::instructions::RedemptionRequest;
use rwa_exit::state::MarketStats;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::{
    account::Account,
    clock::Clock,
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    signature::{keypair_from_seed, Keypair},
    signer::Signer,
    transaction::Transaction,
};
use std::convert::TryInto;
use std::sync::atomic::{AtomicU64, Ordering};

mod basics;
mod builders_buffer;
mod builders_configurable_vault;
mod builders_offer;
mod builders_program;
mod builders_redemption;
mod ed25519;
mod readers;
mod svm;
mod token_accounts;

static DETERMINISTIC_KEYPAIR_ORDINAL: AtomicU64 = AtomicU64::new(0);

pub fn new_test_keypair() -> Keypair {
    let Ok(sample) = std::env::var("PROP_RFQ_RESOURCE_SAMPLE") else {
        return Keypair::new();
    };
    let sample = sample
        .parse::<u64>()
        .expect("PROP_RFQ_RESOURCE_SAMPLE must be an unsigned integer");
    let ordinal = DETERMINISTIC_KEYPAIR_ORDINAL.fetch_add(1, Ordering::Relaxed);
    let mut seed = [0u8; 32];
    seed[..8].copy_from_slice(&(sample + 1).to_le_bytes());
    seed[8..16].copy_from_slice(&(ordinal + 1).to_le_bytes());
    seed[16..24].copy_from_slice(b"RWA-EXIT");
    keypair_from_seed(&seed).expect("deterministic test seed must produce a keypair")
}

pub use basics::*;
pub use builders_buffer::*;
pub use builders_configurable_vault::*;
pub use builders_offer::*;
pub use builders_program::*;
pub use builders_redemption::*;
pub use ed25519::*;
pub use readers::*;
pub use svm::*;
pub use token_accounts::*;
