mod common;

use anchor_lang::{AccountDeserialize, AccountSerialize, AnchorDeserialize};
use common::*;
use rwa_exit::instructions::prop_rfq::{
    apply_hard_wall_liquidity_factor_at_time, apply_hard_wall_reserve_curve_with_params,
    cadence_wave_target_haircut_scaled, cadence_wave_y_for_quote_scaled,
    dynamic_wall_liquidity_at_time, dynamic_wall_position, hard_wall_reserve_from_tvl,
    preview_effective_sell_volume, roll_prop_rfq_volume_tracker, PropRfqPairState, SwapQuote,
    HARD_WALL_SCALE,
};
use rwa_exit::state::ConfigurableVaultKind;
use solana_address_lookup_table_interface::{
    program as lookup_table_program,
    state::{AddressLookupTable, LookupTableMeta},
};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_sdk::account::Account;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::{Transaction, VersionedTransaction};
use std::borrow::Cow;

const ONE_YEAR_SECONDS: u64 = 31_536_000;
const SOLANA_PACKET_DATA_SIZE: usize = 1_232;

struct PropRfqCtx {
    svm: litesvm::LiteSVM,
    payer: Keypair,
    usdc_mint: Pubkey,
    rwa_mint: Pubkey,
    user: Keypair,
}

fn setup_prop_rfq_with_asset_decimals(asset_decimals: u8) -> PropRfqCtx {
    let (mut svm, payer, rwa_mint) = setup_initialized();
    let boss = payer.pubkey();

    let usdc_mint = create_mint(&mut svm, &payer, asset_decimals, &boss);

    let ix = build_make_offer_ix(
        &boss,
        &usdc_mint,
        &rwa_mint,
        0,
        false,
        true,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();

    let (offer_pda, _) = find_offer_pda(&usdc_mint, &rwa_mint);
    let ix = build_set_main_offer_ix(&boss, &offer_pda);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let ix = build_configure_prop_rfq_ix(&boss, &usdc_mint, &rwa_mint, true, 700, 25_000);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();

    let (vault_authority, _) = find_offer_vault_authority_pda();
    let (permissionless_authority, _) = find_permissionless_authority_pda();
    create_token_account(&mut svm, &usdc_mint, &vault_authority, 0);
    create_token_account(&mut svm, &rwa_mint, &vault_authority, 10_000_000_000_000);
    create_token_account(&mut svm, &usdc_mint, &permissionless_authority, 0);
    create_token_account(&mut svm, &rwa_mint, &permissionless_authority, 0);

    let user = new_test_keypair();
    svm.airdrop(&user.pubkey(), 10 * INITIAL_LAMPORTS).unwrap();
    create_token_account(&mut svm, &usdc_mint, &user.pubkey(), 10_000_000_000);
    create_token_account(&mut svm, &usdc_mint, &boss, 0);

    PropRfqCtx {
        svm,
        payer,
        usdc_mint,
        rwa_mint,
        user,
    }
}

fn setup_prop_rfq() -> PropRfqCtx {
    setup_prop_rfq_with_asset_decimals(6)
}

#[derive(Debug)]
struct TransactionShape {
    instruction_accounts: usize,
    unique_legacy_accounts: usize,
    legacy_wire_bytes: usize,
    v0_lookup_wire_bytes: usize,
}

fn compact_length_bytes(value: usize) -> usize {
    if value < 1 << 7 {
        1
    } else if value < 1 << 14 {
        2
    } else {
        3
    }
}

fn transaction_wire_bytes(signature_count: usize, message_bytes: usize) -> usize {
    compact_length_bytes(signature_count) + signature_count * 64 + message_bytes
}

fn lookup_addresses(instruction: &Instruction) -> Vec<Pubkey> {
    let mut addresses = Vec::new();
    for account in &instruction.accounts {
        if !account.is_signer && !addresses.contains(&account.pubkey) {
            addresses.push(account.pubkey);
        }
    }
    addresses
}

fn transaction_shape(
    instruction: &Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
    recent_blockhash: solana_sdk::hash::Hash,
) -> TransactionShape {
    let instructions = [
        ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
        instruction.clone(),
    ];
    let legacy_message = Message::new(&instructions, Some(&payer.pubkey()));
    let legacy_transaction = Transaction::new(signers, legacy_message, recent_blockhash);
    let legacy_wire_bytes = transaction_wire_bytes(
        legacy_transaction.signatures.len(),
        legacy_transaction.message_data().len(),
    );

    let lookup_table = AddressLookupTableAccount {
        key: Pubkey::new_unique(),
        addresses: lookup_addresses(instruction),
    };
    let v0_message = v0::Message::try_compile(
        &payer.pubkey(),
        &instructions,
        &[lookup_table],
        recent_blockhash,
    )
    .unwrap();
    let v0_transaction =
        VersionedTransaction::try_new(VersionedMessage::V0(v0_message), signers).unwrap();
    let v0_lookup_wire_bytes = transaction_wire_bytes(
        v0_transaction.signatures.len(),
        v0_transaction.message.serialize().len(),
    );

    TransactionShape {
        instruction_accounts: instruction.accounts.len(),
        unique_legacy_accounts: legacy_transaction.message.account_keys.len(),
        legacy_wire_bytes,
        v0_lookup_wire_bytes,
    }
}

#[allow(clippy::result_large_err)]
fn send_v0_lookup_tx(
    svm: &mut litesvm::LiteSVM,
    instruction: &Instruction,
    signers: &[&Keypair],
) -> Result<litesvm::types::TransactionMetadata, litesvm::types::FailedTransactionMetadata> {
    if svm.get_sysvar::<solana_sdk::clock::Clock>().slot == 0 {
        advance_slot(svm);
    }
    let payer = signers[0].pubkey();
    let current_slot = svm.get_sysvar::<solana_sdk::clock::Clock>().slot;
    let addresses = lookup_addresses(instruction);
    let lookup_table_key = Pubkey::new_unique();
    let lookup_table_data = AddressLookupTable {
        meta: LookupTableMeta {
            last_extended_slot: current_slot.saturating_sub(1),
            authority: Some(payer),
            ..LookupTableMeta::default()
        },
        addresses: Cow::Owned(addresses.clone()),
    }
    .serialize_for_tests()
    .unwrap();
    svm.set_account(
        lookup_table_key,
        Account {
            lamports: 1_000_000_000,
            data: lookup_table_data,
            owner: lookup_table_program::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let instructions = [
        ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
        instruction.clone(),
    ];
    let message = v0::Message::try_compile(
        &payer,
        &instructions,
        &[AddressLookupTableAccount {
            key: lookup_table_key,
            addresses,
        }],
        svm.latest_blockhash(),
    )
    .unwrap();
    let transaction =
        VersionedTransaction::try_new(VersionedMessage::V0(message), signers).unwrap();
    svm.send_transaction(transaction)
}

fn print_resource_profile(
    name: &str,
    instruction: &Instruction,
    payer: &Keypair,
    signers: &[&Keypair],
    metadata: &litesvm::types::TransactionMetadata,
    recent_blockhash: solana_sdk::hash::Hash,
) {
    let shape = transaction_shape(instruction, payer, signers, recent_blockhash);
    assert!(
        shape.v0_lookup_wire_bytes <= SOLANA_PACKET_DATA_SIZE,
        "{name} v0 transaction is {} bytes, above the {}-byte packet limit",
        shape.v0_lookup_wire_bytes,
        SOLANA_PACKET_DATA_SIZE
    );
    println!(
        "RESOURCE_PROFILE,{name},{},{},{},{},{},{},{}",
        metadata.compute_units_consumed,
        shape.instruction_accounts,
        shape.unique_legacy_accounts,
        shape.legacy_wire_bytes,
        shape.legacy_wire_bytes <= SOLANA_PACKET_DATA_SIZE,
        shape.v0_lookup_wire_bytes,
        shape.v0_lookup_wire_bytes <= SOLANA_PACKET_DATA_SIZE
    );
}

#[allow(clippy::too_many_arguments)]
fn build_configure_prop_rfq_with_params_ix(
    boss: &Pubkey,
    asset_mint: &Pubkey,
    rwa_mint: &Pubkey,
    enabled: bool,
    curve_peg_haircut_bps: u16,
    curve_exponent_scaled: u32,
    cadence_threshold: u32,
    cadence_wave_scaled: u32,
    epoch_duration_seconds: i64,
    wall_sensitivity_scaled: u32,
    minimum_sell_haircut_rwa: u64,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (offer_pda, _) = find_offer_pda(asset_mint, rwa_mint);
    let (prop_rfq_pair_state_pda, _) = find_prop_rfq_pair_state_pda(&offer_pda);
    let mut data = ix_discriminator("configure_prop_rfq").to_vec();
    data.push(if enabled { 1 } else { 0 });
    data.extend_from_slice(&curve_peg_haircut_bps.to_le_bytes());
    data.extend_from_slice(&curve_exponent_scaled.to_le_bytes());
    data.extend_from_slice(&cadence_threshold.to_le_bytes());
    data.extend_from_slice(&cadence_wave_scaled.to_le_bytes());
    data.extend_from_slice(&epoch_duration_seconds.to_le_bytes());
    data.extend_from_slice(&wall_sensitivity_scaled.to_le_bytes());
    data.extend_from_slice(&minimum_sell_haircut_rwa.to_le_bytes());
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new_readonly(offer_pda, false),
            AccountMeta::new_readonly(*asset_mint, false),
            AccountMeta::new(prop_rfq_pair_state_pda, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

fn read_prop_rfq_pair_state(svm: &litesvm::LiteSVM, offer: &Pubkey) -> PropRfqPairState {
    let (pair_state_pda, _) = find_prop_rfq_pair_state_pda(offer);
    let account = svm
        .get_account(&pair_state_pda)
        .expect("Prop RFQ pair state not found");
    let mut data = account.data.as_slice();
    PropRfqPairState::try_deserialize(&mut data).expect("failed to deserialize Prop RFQ pair state")
}

fn write_prop_rfq_pair_state(svm: &mut litesvm::LiteSVM, offer: &Pubkey, state: &PropRfqPairState) {
    let (pair_state_pda, _) = find_prop_rfq_pair_state_pda(offer);
    let mut account = svm
        .get_account(&pair_state_pda)
        .expect("Prop RFQ pair state not found");
    let mut data = Vec::new();
    state
        .try_serialize(&mut data)
        .expect("failed to serialize Prop RFQ pair state");
    account.data = data;
    svm.set_account(pair_state_pda, account).unwrap();
}

fn overwrite_pair_state_pubkey(svm: &mut litesvm::LiteSVM, offer: &Pubkey, offset: usize) {
    let (pair_state_pda, _) = find_prop_rfq_pair_state_pda(offer);
    let mut account = svm
        .get_account(&pair_state_pda)
        .expect("Prop RFQ pair state not found");
    account.data[offset..offset + 32].copy_from_slice(Pubkey::new_unique().as_ref());
    svm.set_account(pair_state_pda, account).unwrap();
}

fn get_balance_or_zero(svm: &litesvm::LiteSVM, ata: &Pubkey) -> u64 {
    if svm.get_account(ata).is_some() {
        get_token_balance(svm, ata)
    } else {
        0
    }
}

fn set_raw_token_balance(svm: &mut litesvm::LiteSVM, token_account: &Pubkey, amount: u64) {
    let mut account = svm
        .get_account(token_account)
        .expect("token account not found");
    account.data[64..72].copy_from_slice(&amount.to_le_bytes());
    svm.set_account(*token_account, account).unwrap();
}

fn add_prop_rfq_vector(ctx: &mut PropRfqCtx) {
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);
    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
}

fn configure_minimum_sell_haircut(ctx: &mut PropRfqCtx, minimum_sell_haircut_rwa: u64) {
    let boss = ctx.payer.pubkey();
    let ix = build_configure_prop_rfq_with_params_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        true,
        700,
        25_000,
        20,
        10_000,
        86_400,
        20_000,
        minimum_sell_haircut_rwa,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
}

#[derive(Debug)]
struct OracleVector<'a> {
    name: &'a str,
    raw_value: u64,
    actual_liquidity: u64,
    haircut_bps: u16,
    exponent_scaled: u32,
    wall_sensitivity_scaled: u32,
    cadence_threshold: u32,
    cadence_wave_scaled: u32,
    epoch_duration: i64,
    elapsed: i64,
    curr_sell: u64,
    curr_buy: u64,
    prev_net_sell: u64,
    sell_count: u32,
    expected_payout: u64,
    tolerance: u64,
}

impl<'a> OracleVector<'a> {
    fn parse(line: &'a str) -> Self {
        let columns = line.split(',').collect::<Vec<_>>();
        assert_eq!(columns.len(), 16, "invalid oracle vector row: {line}");
        Self {
            name: columns[0],
            raw_value: columns[1].parse().unwrap(),
            actual_liquidity: columns[2].parse().unwrap(),
            haircut_bps: columns[3].parse().unwrap(),
            exponent_scaled: columns[4].parse().unwrap(),
            wall_sensitivity_scaled: columns[5].parse().unwrap(),
            cadence_threshold: columns[6].parse().unwrap(),
            cadence_wave_scaled: columns[7].parse().unwrap(),
            epoch_duration: columns[8].parse().unwrap(),
            elapsed: columns[9].parse().unwrap(),
            curr_sell: columns[10].parse().unwrap(),
            curr_buy: columns[11].parse().unwrap(),
            prev_net_sell: columns[12].parse().unwrap(),
            sell_count: columns[13].parse().unwrap(),
            expected_payout: columns[14].parse().unwrap(),
            tolerance: columns[15].parse().unwrap(),
        }
    }
}

#[derive(Debug)]
struct DecimalOracleVector<'a> {
    name: &'a str,
    token_input: u64,
    price_scaled: u64,
    token_in_decimals: u8,
    token_out_decimals: u8,
    expected_raw_value: u64,
}

impl<'a> DecimalOracleVector<'a> {
    fn parse(line: &'a str) -> Self {
        let columns = line.split(',').collect::<Vec<_>>();
        assert_eq!(
            columns.len(),
            6,
            "invalid decimal oracle vector row: {line}"
        );
        Self {
            name: columns[0],
            token_input: columns[1].parse().unwrap(),
            price_scaled: columns[2].parse().unwrap(),
            token_in_decimals: columns[3].parse().unwrap(),
            token_out_decimals: columns[4].parse().unwrap(),
            expected_raw_value: columns[5].parse().unwrap(),
        }
    }
}

fn prepare_prop_rfq_sell_side(ctx: &mut PropRfqCtx, redemption_fee_bps: u16) {
    let boss = ctx.payer.pubkey();
    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    add_prop_rfq_vector(ctx);
    configure_minimum_sell_haircut(ctx, 0);

    if redemption_fee_bps > 0 {
        let ix = build_make_redemption_offer_ix(
            &boss,
            &ctx.rwa_mint,
            &ctx.usdc_mint,
            redemption_fee_bps,
            &TOKEN_PROGRAM_ID,
            &TOKEN_PROGRAM_ID,
        );
        send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    }

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
}

#[test]
fn test_sbf_quotes_match_independent_oracle_vectors() {
    let mut ctx = setup_prop_rfq();
    prepare_prop_rfq_sell_side(&mut ctx, 0);
    let boss = ctx.payer.pubkey();
    let now = get_clock_time(&ctx.svm) as i64;
    let (offer, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    let redemption_vault = derive_ata(
        &redemption_vault_authority,
        &ctx.usdc_mint,
        &TOKEN_PROGRAM_ID,
    );
    let vectors = include_str!("fixtures/prop_rfq_oracle_vectors.csv");
    let mut tested = 0_usize;
    let mut maximum_difference = 0_u64;
    let mut worst_vector = "";

    for line in vectors.lines().skip(1).filter(|line| !line.is_empty()) {
        let vector = OracleVector::parse(line);
        advance_slot(&mut ctx.svm);
        let ix = build_configure_prop_rfq_with_params_ix(
            &boss,
            &ctx.usdc_mint,
            &ctx.rwa_mint,
            true,
            vector.haircut_bps,
            vector.exponent_scaled,
            vector.cadence_threshold,
            vector.cadence_wave_scaled,
            vector.epoch_duration,
            vector.wall_sensitivity_scaled.max(1),
            0,
        );
        send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

        let mut state = read_prop_rfq_pair_state(&ctx.svm, &offer);
        state.wall_sensitivity_scaled = vector.wall_sensitivity_scaled;
        state.curr_sell_value_stable = vector.curr_sell;
        state.curr_buy_value_stable = vector.curr_buy;
        state.prev_net_sell_value_stable = vector.prev_net_sell;
        state.curr_sell_trade_count = vector.sell_count;
        state.epoch_start = if vector.elapsed >= 0 {
            now.checked_sub(vector.elapsed).unwrap()
        } else {
            now.checked_add(vector.elapsed.unsigned_abs() as i64)
                .unwrap()
        };
        write_prop_rfq_pair_state(&mut ctx.svm, &offer, &state);
        set_raw_token_balance(&mut ctx.svm, &redemption_vault, vector.actual_liquidity);

        advance_slot(&mut ctx.svm);
        let token_in_amount = vector.raw_value.checked_mul(1_000).unwrap();
        let quote_ix = build_quote_swap_ix(
            &ctx.rwa_mint,
            &ctx.rwa_mint,
            &ctx.usdc_mint,
            token_in_amount,
        );
        let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer])
            .unwrap_or_else(|error| panic!("{}: quote failed: {error:?}", vector.name));
        let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
        let difference = quote.token_out_amount.abs_diff(vector.expected_payout);
        if difference > maximum_difference {
            maximum_difference = difference;
            worst_vector = vector.name;
        }
        assert!(
            difference <= vector.tolerance,
            "{}: SBF payout {} differs from independent oracle {} by {}, tolerance {}",
            vector.name,
            quote.token_out_amount,
            vector.expected_payout,
            difference,
            vector.tolerance,
        );
        tested += 1;
    }

    assert_eq!(tested, 34);
    println!(
        "independent oracle vectors: {tested}, maximum absolute difference: {maximum_difference} base units, worst vector: {worst_vector}"
    );

    set_raw_token_balance(&mut ctx.svm, &redemption_vault, 0);
    advance_slot(&mut ctx.svm);
    let zero_liquidity_quote =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, 1_000);
    assert!(send_tx(&mut ctx.svm, &[zero_liquidity_quote], &[&ctx.payer]).is_err());

    set_raw_token_balance(&mut ctx.svm, &redemption_vault, 1);
    advance_slot(&mut ctx.svm);
    let above_liquidity_quote =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, 2_000);
    assert!(send_tx(&mut ctx.svm, &[above_liquidity_quote], &[&ctx.payer]).is_err());

    set_raw_token_balance(&mut ctx.svm, &redemption_vault, 10_000_000_000);
    advance_slot(&mut ctx.svm);
    let zero_raw_quote = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, 0);
    assert!(send_tx(&mut ctx.svm, &[zero_raw_quote], &[&ctx.payer]).is_err());
}

#[test]
fn test_sbf_quotes_match_independent_decimal_vectors() {
    let vectors = include_str!("fixtures/prop_rfq_decimal_vectors.csv");
    let mut tested = 0_usize;

    for line in vectors.lines().skip(1).filter(|line| !line.is_empty()) {
        let vector = DecimalOracleVector::parse(line);
        assert_eq!(vector.price_scaled, 1_000_000_000);
        assert_eq!(vector.token_in_decimals, 9);
        let mut ctx = setup_prop_rfq_with_asset_decimals(vector.token_out_decimals);
        prepare_prop_rfq_sell_side(&mut ctx, 0);
        let boss = ctx.payer.pubkey();
        let ix = build_configure_prop_rfq_with_params_ix(
            &boss,
            &ctx.usdc_mint,
            &ctx.rwa_mint,
            true,
            0,
            10_000,
            20,
            0,
            86_400,
            1,
            0,
        );
        send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

        advance_slot(&mut ctx.svm);
        let quote_ix = build_quote_swap_ix(
            &ctx.rwa_mint,
            &ctx.rwa_mint,
            &ctx.usdc_mint,
            vector.token_input,
        );
        let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer])
            .unwrap_or_else(|error| panic!("{}: quote failed: {error:?}", vector.name));
        let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
        assert_eq!(
            quote.token_out_amount, vector.expected_raw_value,
            "{}: decimal conversion mismatch",
            vector.name
        );
        tested += 1;
    }

    assert_eq!(tested, 5);
}

fn initialize_prop_rfq_buffer(ctx: &mut PropRfqCtx, gross_yield: u64) {
    let boss = ctx.payer.pubkey();
    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    let ix = build_initialize_buffer_ix(&boss, &offer_pda, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_set_buffer_gross_yield_ix(&boss, &offer_pda, &ctx.rwa_mint, gross_yield);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
}

#[test]
fn test_hard_wall_curve_is_vulnerable_to_order_splitting() {
    let hard_wall_reserve = 10_000_000;
    let one_shot = apply_hard_wall_reserve_curve_with_params(
        5_000_000,
        10_000_000,
        hard_wall_reserve,
        700,
        25_000,
    )
    .unwrap();

    let mut split_total = 0_u64;
    let mut current_liquidity = 10_000_000_u64;
    for _ in 0..5 {
        let output = apply_hard_wall_reserve_curve_with_params(
            1_000_000,
            current_liquidity,
            hard_wall_reserve,
            700,
            25_000,
        )
        .unwrap();
        split_total += output;
        current_liquidity -= output;
    }

    assert_eq!(one_shot, 4_938_128);
    assert_eq!(split_total, 4_997_768);
}

#[test]
fn test_hard_wall_curve_approximation_tracks_exact_curve() {
    let actual_liquidity = 50_000_000;
    let hard_wall_reserve = 1_000_000;
    let peg_haircut_bps = 700;
    let exponents = [
        1_000_u32, 2_000, 5_000, 10_000, 15_000, 20_000, 24_000, 25_000, 30_000, 50_000, 100_000,
    ];
    let token_out_amounts = [
        1_000_u64, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 800_000, 1_000_000,
        1_250_000, 1_500_000, 2_000_000, 3_000_000, 5_000_000, 10_000_000, 25_000_000, 50_000_000,
    ];

    for exponent in exponents {
        for token_out_amount in token_out_amounts {
            let approximate = apply_hard_wall_reserve_curve_with_params(
                token_out_amount,
                actual_liquidity,
                hard_wall_reserve,
                peg_haircut_bps,
                exponent,
            )
            .unwrap();
            let exact = exact_hard_wall_output(
                token_out_amount,
                hard_wall_reserve,
                peg_haircut_bps,
                exponent,
            );
            assert!(
                approximate.abs_diff(exact) <= 2,
                "approximation drifted for token_out={token_out_amount}, exponent={exponent}: approximate={approximate}, exact={exact}"
            );
        }
    }
}

#[test]
fn test_hard_wall_curve_ignores_surplus_above_target_reserve() {
    let hard_wall_reserve = 5_000_000;
    let raw_sell_value_stable = 1_000_000;
    let at_target = apply_hard_wall_reserve_curve_with_params(
        raw_sell_value_stable,
        hard_wall_reserve,
        hard_wall_reserve,
        700,
        25_000,
    )
    .unwrap();
    let above_target = apply_hard_wall_reserve_curve_with_params(
        raw_sell_value_stable,
        10_000_000,
        hard_wall_reserve,
        700,
        25_000,
    )
    .unwrap();

    assert_eq!(above_target, at_target);
}

fn exact_hard_wall_output(
    token_out_amount: u64,
    effective_liquidity: u64,
    curve_peg_haircut_bps: u16,
    curve_exponent_scaled: u32,
) -> u64 {
    let utilization = (token_out_amount as u128)
        .checked_mul(HARD_WALL_SCALE)
        .unwrap()
        .checked_div(effective_liquidity as u128)
        .unwrap();
    let peg_haircut = HARD_WALL_SCALE
        .checked_mul(curve_peg_haircut_bps as u128)
        .unwrap()
        .checked_div(10_000)
        .unwrap();
    let utilization_power = exact_utilization_power_scaled(utilization, curve_exponent_scaled);
    let haircut = peg_haircut
        .saturating_mul(utilization_power)
        .checked_div(HARD_WALL_SCALE)
        .unwrap();
    let liquidity_factor = HARD_WALL_SCALE.saturating_sub(haircut);
    let dampened = (token_out_amount as u128)
        .checked_mul(liquidity_factor)
        .unwrap()
        .checked_div(HARD_WALL_SCALE)
        .unwrap();
    dampened as u64
}

fn exact_utilization_power_scaled(u: u128, exponent_scaled: u32) -> u128 {
    if exponent_scaled == 0 {
        return HARD_WALL_SCALE;
    }

    let tenths = exponent_scaled / 1_000;
    let tenth_root = exact_tenth_root_scaled(u);
    let mut value = HARD_WALL_SCALE;
    for _ in 0..tenths {
        value = exact_mul_scaled(value, tenth_root);
    }
    value
}

fn exact_tenth_root_scaled(value: u128) -> u128 {
    if value <= 1 || value == HARD_WALL_SCALE {
        return value;
    }

    let mut left = 1_u128;
    let mut right = value.max(HARD_WALL_SCALE);
    let mut answer = 1_u128;
    while left <= right {
        let mid = left + (right - left) / 2;
        if exact_pow_scaled_lte(mid, 10, value) {
            answer = mid;
            left = mid + 1;
        } else {
            right = mid - 1;
        }
    }
    answer
}

fn exact_pow_scaled_lte(base: u128, exponent: u32, limit: u128) -> bool {
    let mut value = HARD_WALL_SCALE;
    if base >= HARD_WALL_SCALE {
        for _ in 0..exponent {
            value = exact_mul_scaled(value, base);
            if value > limit {
                return false;
            }
        }
        return true;
    }

    for _ in 0..exponent {
        value = exact_mul_scaled(value, base);
        if value <= limit {
            return true;
        }
    }
    value <= limit
}

fn exact_mul_scaled(lhs: u128, rhs: u128) -> u128 {
    lhs.saturating_mul(rhs)
        .checked_div(HARD_WALL_SCALE)
        .unwrap_or(u128::MAX)
}

#[test]
fn test_hard_wall_curve_allows_zero_output_at_actual_vault_limit() {
    let state = PropRfqPairState {
        curve_peg_haircut_bps: 700,
        curve_exponent_scaled: 25_000,
        cadence_threshold: 20,
        cadence_wave_scaled: 10_000,
        epoch_duration_seconds: 86_400,
        wall_sensitivity_scaled: 20_000,
        curr_sell_value_stable: 0,
        curr_buy_value_stable: 0,
        prev_net_sell_value_stable: 0,
        curr_sell_trade_count: 0,
        epoch_start: 1,
        bump: 0,
        ..Default::default()
    };
    let output =
        apply_hard_wall_liquidity_factor_at_time(10_000_000, 10_000_000, 10_000_000, &state, 1)
            .unwrap();

    assert_eq!(output, 0);
}

#[test]
fn test_hard_wall_curve_saturates_extreme_utilization() {
    let output = apply_hard_wall_reserve_curve_with_params(
        1_000_000_000_000,
        1_000_000_000_000,
        1,
        1,
        32_000,
    )
    .unwrap();

    assert_eq!(output, 0);
}

#[test]
fn test_hard_wall_curve_rejects_raw_value_above_actual_vault() {
    let result =
        apply_hard_wall_reserve_curve_with_params(10_000_001, 10_000_000, 10_000_000, 700, 25_000);

    assert!(result.is_err());
}

#[test]
fn test_hard_wall_liquidity_rejects_output_above_actual_liquidity() {
    let state = PropRfqPairState {
        curve_peg_haircut_bps: 700,
        curve_exponent_scaled: 25_000,
        cadence_threshold: 20,
        cadence_wave_scaled: 10_000,
        epoch_duration_seconds: 86_400,
        wall_sensitivity_scaled: 20_000,
        epoch_start: 1,
        bump: 0,
        ..Default::default()
    };

    let result =
        apply_hard_wall_liquidity_factor_at_time(10_000_001, 10_000_000, 10_000_000, &state, 1);

    assert!(result.is_err());
}

#[test]
fn test_hard_wall_reserve_from_tvl_scales_to_token_out_decimals() {
    assert_eq!(
        hard_wall_reserve_from_tvl(2_000_000_000_000, 1_500, 6, 9).unwrap(),
        300_000_000
    );
    assert!(hard_wall_reserve_from_tvl(1, 1, 6, 9).is_err());
}

#[test]
fn test_dynamic_wall_preview_includes_current_sell_and_buy_relief() {
    let state = PropRfqPairState {
        curve_peg_haircut_bps: 700,
        curve_exponent_scaled: 25_000,
        cadence_threshold: 20,
        cadence_wave_scaled: 10_000,
        epoch_duration_seconds: 86_400,
        wall_sensitivity_scaled: 20_000,
        curr_sell_value_stable: 500,
        curr_buy_value_stable: 100,
        prev_net_sell_value_stable: 1_000,
        curr_sell_trade_count: 0,
        epoch_start: 1_000,
        bump: 0,
        ..Default::default()
    };

    let effective = preview_effective_sell_volume(&state, 200, 44_200).unwrap();
    assert_eq!(effective, 1_100);
}

#[test]
fn test_dynamic_wall_position_uses_effective_sell_pressure() {
    assert_eq!(
        dynamic_wall_position(15_000_000, 0, 20_000).unwrap(),
        15_000_000
    );
    assert_eq!(
        dynamic_wall_position(15_000_000, 15_000_000, 20_000).unwrap(),
        5_000_000
    );
    assert_eq!(
        dynamic_wall_position(15_000_000, 30_000_000, 20_000).unwrap(),
        3_000_000
    );
}

#[test]
fn test_dynamic_wall_liquidity_matches_graph_dynamic_wall() {
    let state = PropRfqPairState {
        epoch_duration_seconds: 86_400,
        wall_sensitivity_scaled: 20_000,
        epoch_start: 1,
        ..Default::default()
    };

    let liquidity =
        dynamic_wall_liquidity_at_time(100_000, 10_000_000_000, 20_000_000_000, &state, 1).unwrap();

    assert_eq!(liquidity, 10_000_000_000);

    let capped_liquidity =
        dynamic_wall_liquidity_at_time(100_000, 10_000_000_000, 200_000, &state, 1).unwrap();

    assert_eq!(capped_liquidity, 200_000);
}

#[test]
fn test_prop_rfq_volume_tracker_rolls_and_resets_epochs() {
    let mut state = PropRfqPairState {
        epoch_duration_seconds: 100,
        epoch_start: 1_000,
        curr_sell_value_stable: 1_000,
        curr_buy_value_stable: 250,
        prev_net_sell_value_stable: 125,
        curr_sell_trade_count: 7,
        ..Default::default()
    };

    roll_prop_rfq_volume_tracker(&mut state, 1_100).unwrap();
    assert_eq!(state.epoch_start, 1_100);
    assert_eq!(state.prev_net_sell_value_stable, 750);
    assert_eq!(state.curr_sell_value_stable, 0);
    assert_eq!(state.curr_buy_value_stable, 0);
    assert_eq!(state.curr_sell_trade_count, 0);

    state.curr_sell_value_stable = 500;
    state.curr_buy_value_stable = 100;
    state.curr_sell_trade_count = 3;
    roll_prop_rfq_volume_tracker(&mut state, 1_300).unwrap();
    assert_eq!(state.epoch_start, 1_300);
    assert_eq!(state.prev_net_sell_value_stable, 0);
    assert_eq!(state.curr_sell_value_stable, 0);
    assert_eq!(state.curr_buy_value_stable, 0);
    assert_eq!(state.curr_sell_trade_count, 0);
}

#[test]
fn test_prop_rfq_recovery_uses_the_actual_epoch_boundary() {
    let mut state = PropRfqPairState {
        epoch_duration_seconds: 100,
        epoch_start: 1_000,
        curr_sell_value_stable: 1_000,
        curr_buy_value_stable: 250,
        ..Default::default()
    };

    let preview = preview_effective_sell_volume(&state, 100, 1_150).unwrap();
    assert_eq!(preview, 475);

    roll_prop_rfq_volume_tracker(&mut state, 1_150).unwrap();
    assert_eq!(state.epoch_start, 1_100);
    assert_eq!(state.prev_net_sell_value_stable, 750);
    assert_eq!(
        preview_effective_sell_volume(&state, 100, 1_150).unwrap(),
        preview
    );
}

#[test]
fn test_cadence_wave_y_ramps_to_configured_maximum() {
    let state = PropRfqPairState {
        curve_peg_haircut_bps: 7_000,
        curve_exponent_scaled: 25_000,
        cadence_threshold: 20,
        cadence_wave_scaled: 10_000,
        epoch_duration_seconds: 86_400,
        wall_sensitivity_scaled: 20_000,
        curr_sell_value_stable: 0,
        curr_buy_value_stable: 0,
        prev_net_sell_value_stable: 0,
        curr_sell_trade_count: 0,
        epoch_start: 1,
        bump: 0,
        ..Default::default()
    };
    let mut half_cadence = state.clone();
    half_cadence.curr_sell_trade_count = 10;
    let mut high_cadence = state.clone();
    high_cadence.curr_sell_trade_count = 49;
    let mut threshold_cadence = state.clone();
    threshold_cadence.curr_sell_trade_count = 20;

    assert_eq!(cadence_wave_y_for_quote_scaled(&state, 1).unwrap(), 0);
    assert_eq!(
        cadence_wave_y_for_quote_scaled(&half_cadence, 1).unwrap(),
        5_000
    );
    assert_eq!(
        cadence_wave_y_for_quote_scaled(&threshold_cadence, 1).unwrap(),
        10_000
    );
    assert_eq!(
        cadence_wave_y_for_quote_scaled(&high_cadence, 1).unwrap(),
        10_000
    );

    let mut invalid_legacy_state = state;
    invalid_legacy_state.cadence_wave_scaled = 100_000;
    assert!(cadence_wave_y_for_quote_scaled(&invalid_legacy_state, 1).is_err());
}

#[test]
fn test_cadence_wave_target_matches_explorer_integer_vectors() {
    let vectors = [
        (0, 0),
        (10_000_000_000, 24_922_118_380),
        (100_000_000_000, 156_862_745_098),
        (250_000_000_000, 242_424_242_424),
        (500_000_000_000, 296_296_296_296),
        (750_000_000_000, 320_000_000_000),
        (1_000_000_000_000, 333_333_333_333),
        (2_000_000_000_000, 333_333_333_333),
    ];

    for (utilization, expected_haircut) in vectors {
        assert_eq!(
            cadence_wave_target_haircut_scaled(utilization, 10_000).unwrap(),
            expected_haircut,
            "explorer mismatch at utilization={utilization}"
        );
    }
    assert_eq!(
        cadence_wave_target_haircut_scaled(250_000_000_000, 50_000).unwrap(),
        HARD_WALL_SCALE
    );
}

#[test]
fn test_cadence_penalizes_small_split_sells() {
    let state = PropRfqPairState {
        curve_peg_haircut_bps: 7_000,
        curve_exponent_scaled: 25_000,
        cadence_threshold: 20,
        cadence_wave_scaled: 10_000,
        epoch_duration_seconds: 86_400,
        wall_sensitivity_scaled: 0,
        curr_sell_value_stable: 0,
        curr_buy_value_stable: 0,
        prev_net_sell_value_stable: 0,
        curr_sell_trade_count: 0,
        epoch_start: 1,
        bump: 0,
        ..Default::default()
    };
    let mut high_cadence = state.clone();
    high_cadence.curr_sell_trade_count = 49;

    let low_cadence_output =
        apply_hard_wall_liquidity_factor_at_time(100_000, 10_000_000, 10_000_000, &state, 1)
            .unwrap();
    let high_cadence_output =
        apply_hard_wall_liquidity_factor_at_time(100_000, 10_000_000, 10_000_000, &high_cadence, 1)
            .unwrap();

    assert_eq!(low_cadence_output, 99_999);
    assert_eq!(high_cadence_output, 97_507);
}

#[test]
fn test_quote_swap_returns_expected_quote_data() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    let metadata = send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&metadata)).unwrap();

    assert_eq!(quote.offer, find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint).0);
    assert_eq!(quote.token_in_amount, 1_000_000);
    assert_eq!(quote.token_in_net_amount, 1_000_000);
    assert_eq!(quote.token_in_fee_amount, 0);
    assert_eq!(quote.token_out_amount, 1_000_000_000);
    assert_eq!(quote.minimum_out, quote.token_out_amount);
}

#[test]
fn test_quote_swap_rejects_invalid_token_pairs() {
    let mut ctx = setup_prop_rfq();
    let eurc_mint = create_mint(&mut ctx.svm, &ctx.payer, 6, &ctx.payer.pubkey());

    let mut same_mint_ix =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    same_mint_ix.accounts[4] = AccountMeta::new_readonly(ctx.usdc_mint, false);
    assert!(
        send_tx(&mut ctx.svm, &[same_mint_ix], &[&ctx.payer]).is_err(),
        "quote should reject identical token in/out mints"
    );

    let mut no_rwa_ix =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    no_rwa_ix.accounts[4] = AccountMeta::new_readonly(eurc_mint, false);
    assert!(
        send_tx(&mut ctx.svm, &[no_rwa_ix], &[&ctx.payer]).is_err(),
        "quote should reject pairs that do not include RWA"
    );
}

#[test]
fn test_dynamic_wall_ignores_buys_without_redemption_vault_refill() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    configure_minimum_sell_haircut(&mut ctx, 0);

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    let (pair_state_pda, _) = find_prop_rfq_pair_state_pda(&offer_pda);
    let pair_state_account = ctx.svm.get_account(&pair_state_pda).unwrap();
    println!(
        "RESOURCE_ACCOUNT,prop_rfq_pair_state,{},{}",
        pair_state_account.data.len(),
        pair_state_account.lamports
    );

    let sell_amount = 2_000_000_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_blockhash = ctx.svm.latest_blockhash();
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    print_resource_profile(
        "quote_sell",
        &build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount),
        &ctx.payer,
        &[&ctx.payer],
        &quote_metadata,
        quote_blockhash,
    );
    let first_quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    let sell_ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        first_quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    let sell_blockhash = ctx.svm.latest_blockhash();
    let sell_metadata =
        send_v0_lookup_tx(&mut ctx.svm, &sell_ix, &[&ctx.payer, &ctx.user]).unwrap();
    print_resource_profile(
        "open_swap_sell",
        &sell_ix,
        &ctx.payer,
        &[&ctx.payer, &ctx.user],
        &sell_metadata,
        sell_blockhash,
    );

    advance_slot(&mut ctx.svm);
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let pressured_quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
    assert_eq!(first_quote.token_out_amount, 1_994_192_046);
    assert_eq!(pressured_quote.token_out_amount, 1_970_377_785);

    let buy_quote_ix =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000_000);
    let buy_quote_blockhash = ctx.svm.latest_blockhash();
    let buy_quote_metadata = send_tx(&mut ctx.svm, &[buy_quote_ix], &[&ctx.payer]).unwrap();
    print_resource_profile(
        "quote_swap_buy",
        &build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000_000),
        &ctx.payer,
        &[&ctx.payer],
        &buy_quote_metadata,
        buy_quote_blockhash,
    );
    let buy_quote = SwapQuote::try_from_slice(get_return_data(&buy_quote_metadata)).unwrap();
    let buy_ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000_000,
        buy_quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    let buy_blockhash = ctx.svm.latest_blockhash();
    let buy_metadata = send_v0_lookup_tx(&mut ctx.svm, &buy_ix, &[&ctx.payer, &ctx.user]).unwrap();
    print_resource_profile(
        "open_swap_buy",
        &buy_ix,
        &ctx.payer,
        &[&ctx.payer, &ctx.user],
        &buy_metadata,
        buy_blockhash,
    );

    advance_slot(&mut ctx.svm);
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let unrelieved_quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
    assert_eq!(
        unrelieved_quote.token_out_amount,
        pressured_quote.token_out_amount
    );

    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    assert_eq!(
        read_prop_rfq_pair_state(&ctx.svm, &offer_pda).curr_buy_value_stable,
        0,
        "a buy routed entirely to proceeds must not relieve sell pressure"
    );
}

#[test]
fn test_prop_rfq_pair_must_be_enabled() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();

    let ix = build_configure_prop_rfq_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint, false, 700, 25_000);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    let result = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]);
    assert!(
        result.is_err(),
        "disabled Prop RFQ pair should reject quotes"
    );
}

#[test]
fn test_configure_prop_rfq_rejects_non_boss() {
    let mut ctx = setup_prop_rfq();
    let unauthorized = new_test_keypair();
    ctx.svm
        .airdrop(&unauthorized.pubkey(), INITIAL_LAMPORTS)
        .unwrap();

    let ix = build_configure_prop_rfq_ix(
        &unauthorized.pubkey(),
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        false,
        700,
        25_000,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &[&unauthorized]);
    assert!(result.is_err(), "non-boss should not configure Prop RFQ");

    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    assert!(read_prop_rfq_pair_state(&ctx.svm, &offer_pda).enabled);
}

#[test]
fn test_configure_prop_rfq_rejects_invalid_parameters() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();

    let invalid_cases = [
        (10_001, 25_000, 20, 10_000, 86_400, 20_000),
        (700, 999, 20, 10_000, 86_400, 20_000),
        (700, 100_001, 20, 10_000, 86_400, 20_000),
        (700, 25_500, 20, 10_000, 86_400, 20_000),
        (700, 25_000, 0, 10_000, 86_400, 20_000),
        (700, 25_000, 20, 50_001, 86_400, 20_000),
        (700, 25_000, 20, 1_500, 86_400, 20_000),
        (700, 25_000, 20, 10_000, 0, 20_000),
        (700, 25_000, 20, 10_000, 86_400, 0),
    ];

    for (
        haircut_bps,
        exponent_scaled,
        cadence_threshold,
        cadence_wave_scaled,
        epoch_duration_seconds,
        wall_sensitivity_scaled,
    ) in invalid_cases
    {
        let ix = build_configure_prop_rfq_with_params_ix(
            &boss,
            &ctx.usdc_mint,
            &ctx.rwa_mint,
            true,
            haircut_bps,
            exponent_scaled,
            cadence_threshold,
            cadence_wave_scaled,
            epoch_duration_seconds,
            wall_sensitivity_scaled,
            5_000_000_000,
        );
        assert!(
            send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).is_err(),
            "invalid Prop RFQ config should fail: {invalid_cases:?}"
        );
    }
}

#[test]
fn test_configure_prop_rfq_accepts_max_and_zero_cadence_wave() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();

    let ix = build_configure_prop_rfq_with_params_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        true,
        700,
        25_000,
        7,
        50_000,
        86_400,
        20_000,
        5_000_000_000,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (offer, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    let pair = read_prop_rfq_pair_state(&ctx.svm, &offer);
    assert_eq!(pair.cadence_threshold, 7);
    assert_eq!(pair.cadence_wave_scaled, 50_000);

    let ix = build_configure_prop_rfq_with_params_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        true,
        700,
        25_000,
        3,
        0,
        86_400,
        20_000,
        5_000_000_000,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let pair = read_prop_rfq_pair_state(&ctx.svm, &offer);
    assert_eq!(pair.cadence_threshold, 3);
    assert_eq!(pair.cadence_wave_scaled, 0);
}

#[test]
fn test_prop_rfq_rejects_pair_state_for_different_offer() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let eurc_mint = create_mint(&mut ctx.svm, &ctx.payer, 6, &boss);

    let ix = build_make_offer_ix(
        &boss,
        &eurc_mint,
        &ctx.rwa_mint,
        0,
        false,
        true,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_configure_prop_rfq_ix(&boss, &eurc_mint, &ctx.rwa_mint, true, 1_200, 20_000);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (usdc_offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    let (usdc_pair_state_pda, _) = find_prop_rfq_pair_state_pda(&usdc_offer_pda);
    let mut ix = build_quote_swap_ix(&ctx.rwa_mint, &eurc_mint, &ctx.rwa_mint, 1_000_000);
    ix.accounts[1] = AccountMeta::new_readonly(usdc_pair_state_pda, false);

    let result = send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]);
    assert!(
        result.is_err(),
        "Prop RFQ should reject a pair-state PDA derived from another offer"
    );
}

#[test]
fn test_prop_rfq_rejects_pair_state_with_mismatched_stored_mints() {
    let mut ctx = setup_prop_rfq();
    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);

    overwrite_pair_state_pubkey(&mut ctx.svm, &offer_pda, 8 + 32);
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    assert!(
        send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).is_err(),
        "Prop RFQ should reject a pair state with the wrong stored asset mint"
    );

    let boss = ctx.payer.pubkey();
    let ix = build_configure_prop_rfq_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint, true, 700, 26_000);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    overwrite_pair_state_pubkey(&mut ctx.svm, &offer_pda, 8 + 32 + 32);
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    assert!(
        send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).is_err(),
        "Prop RFQ should reject a pair state with the wrong stored RWA mint"
    );
}

#[test]
fn test_open_swap_enforces_minimum_out() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    let ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        quote.minimum_out + 1,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]);
    assert!(result.is_err());

    let ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let user_rwa = get_token_balance(
        &ctx.svm,
        &get_associated_token_address(&ctx.user.pubkey(), &ctx.rwa_mint),
    );
    assert_eq!(user_rwa, quote.token_out_amount);
}

#[test]
fn test_open_swap_buy_creates_prefunded_user_output_ata() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    add_prop_rfq_vector(&mut ctx);

    let user_rwa_ata = get_associated_token_address(&ctx.user.pubkey(), &ctx.rwa_mint);
    ctx.svm
        .set_account(
            user_rwa_ata,
            Account {
                executable: false,
                data: Vec::new(),
                lamports: 1,
                owner: SYSTEM_PROGRAM_ID,
                rent_epoch: 0,
            },
        )
        .unwrap();

    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    let ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    assert_eq!(
        get_token_balance(&ctx.svm, &user_rwa_ata),
        quote.token_out_amount
    );
}

#[test]
fn test_open_swap_sell_enforces_minimum_out() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    prepare_prop_rfq_sell_side(&mut ctx, 0);

    let sell_amount = 100_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        quote.minimum_out + 1,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    assert!(send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).is_err());
}

#[test]
fn test_open_swap_sell_applies_default_minimum_haircut() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    add_prop_rfq_vector(&mut ctx);

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        20_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let dust_quote_ix =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, 100_000_000);
    assert!(
        send_tx(&mut ctx.svm, &[dust_quote_ix], &[&ctx.payer]).is_err(),
        "gross input below 5 RWA minimum haircut should fail"
    );

    let zero_net_sell_amount = 5_000_000_000;
    let quote_ix = build_quote_swap_ix(
        &ctx.rwa_mint,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        zero_net_sell_amount,
    );
    assert!(
        send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).is_err(),
        "positive input whose minimum haircut leaves zero net amount should fail"
    );

    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        zero_net_sell_amount,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    assert!(
        send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).is_err(),
        "positive input whose minimum haircut leaves zero output should fail"
    );

    let sell_amount = 5_100_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    assert_eq!(quote.token_in_amount, sell_amount);
    assert_eq!(quote.token_in_fee_amount, 5_000_000_000);
    assert_eq!(quote.token_in_net_amount, 100_000_000);

    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let fee_vault_ata =
        get_associated_token_address(&find_prop_rfq_sell_fee_vault_pda().0, &ctx.rwa_mint);
    assert_eq!(get_token_balance(&ctx.svm, &fee_vault_ata), 5_000_000_000);
}

#[test]
fn test_open_swap_sell_uses_prop_rfq_sell_redemption_fee_and_vault() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    prepare_prop_rfq_sell_side(&mut ctx, 100);

    let ix = build_update_redemption_offer_prop_rfq_sell_fee_ix(
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        300,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let sell_amount = 1_000_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
    assert_eq!(quote.token_in_fee_amount, 30_000_000);
    assert_eq!(quote.token_in_net_amount, 970_000_000);

    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let sell_fee_ata =
        get_associated_token_address(&find_prop_rfq_sell_fee_vault_pda().0, &ctx.rwa_mint);
    let old_prop_rfq_buy_fee_ata =
        get_associated_token_address(&find_prop_rfq_buy_fee_vault_pda().0, &ctx.rwa_mint);
    assert_eq!(get_token_balance(&ctx.svm, &sell_fee_ata), 30_000_000);
    assert_eq!(get_balance_or_zero(&ctx.svm, &old_prop_rfq_buy_fee_ata), 0);
}

#[test]
fn test_open_swap_buy_uses_permissionless_offer_fee() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    add_prop_rfq_vector(&mut ctx);

    let ix = build_update_offer_fee_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint, 100);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_update_offer_permissionless_fee_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint, 300);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let buy_amount = 1_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, buy_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
    assert_eq!(quote.token_in_fee_amount, 30_000);
    assert_eq!(quote.token_in_net_amount, 970_000);

    let ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        buy_amount,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let prop_rfq_buy_fee_ata =
        get_associated_token_address(&find_prop_rfq_buy_fee_vault_pda().0, &ctx.usdc_mint);
    assert_eq!(get_token_balance(&ctx.svm, &prop_rfq_buy_fee_ata), 30_000);
}

#[test]
fn test_prop_rfq_rejects_quotes_and_swaps_when_kill_switch_active() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    add_prop_rfq_vector(&mut ctx);

    let ix = build_set_kill_switch_ix(&boss, true);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    assert!(
        send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).is_err(),
        "buy quote should reject while the kill switch is active"
    );

    let buy_ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    assert!(
        send_tx(&mut ctx.svm, &[buy_ix], &[&ctx.payer, &ctx.user]).is_err(),
        "buy execution should reject while the kill switch is active"
    );

    let mut sell_ctx = setup_prop_rfq();
    let sell_boss = sell_ctx.payer.pubkey();
    prepare_prop_rfq_sell_side(&mut sell_ctx, 0);
    let ix = build_set_kill_switch_ix(&sell_boss, true);
    send_tx(&mut sell_ctx.svm, &[ix], &[&sell_ctx.payer]).unwrap();

    let sell_quote_ix = build_quote_swap_ix(
        &sell_ctx.rwa_mint,
        &sell_ctx.rwa_mint,
        &sell_ctx.usdc_mint,
        100,
    );
    assert!(
        send_tx(&mut sell_ctx.svm, &[sell_quote_ix], &[&sell_ctx.payer]).is_err(),
        "sell quote should reject while the kill switch is active"
    );

    let sell_ix = build_open_swap_sell_ix(
        &sell_ctx.rwa_mint,
        &sell_ctx.user.pubkey(),
        &sell_boss,
        &sell_ctx.rwa_mint,
        &sell_ctx.usdc_mint,
        100,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    assert!(
        send_tx(
            &mut sell_ctx.svm,
            &[sell_ix],
            &[&sell_ctx.payer, &sell_ctx.user]
        )
        .is_err(),
        "sell execution should reject while the kill switch is active"
    );
}

#[test]
fn test_open_swap_buy_respects_max_supply_and_max_mint_amount() {
    let mut max_supply_ctx = setup_prop_rfq();
    let boss = max_supply_ctx.payer.pubkey();
    add_prop_rfq_vector(&mut max_supply_ctx);
    let ix = build_transfer_mint_authority_to_program_ix(
        &boss,
        &max_supply_ctx.rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut max_supply_ctx.svm, &[ix], &[&max_supply_ctx.payer]).unwrap();

    let quote_ix = build_quote_swap_ix(
        &max_supply_ctx.rwa_mint,
        &max_supply_ctx.usdc_mint,
        &max_supply_ctx.rwa_mint,
        1_000_000,
    );
    let quote_metadata = send_tx(
        &mut max_supply_ctx.svm,
        &[quote_ix],
        &[&max_supply_ctx.payer],
    )
    .unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();
    let current_supply = get_mint_supply(&max_supply_ctx.svm, &max_supply_ctx.rwa_mint);
    let ix = build_configure_max_supply_ix(&boss, current_supply + quote.token_out_amount - 1);
    send_tx(&mut max_supply_ctx.svm, &[ix], &[&max_supply_ctx.payer]).unwrap();
    let buy_ix = build_open_swap_buy_ix(
        &max_supply_ctx.rwa_mint,
        &max_supply_ctx.user.pubkey(),
        &boss,
        &max_supply_ctx.usdc_mint,
        &max_supply_ctx.rwa_mint,
        1_000_000,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    assert!(
        send_tx(
            &mut max_supply_ctx.svm,
            &[buy_ix],
            &[&max_supply_ctx.payer, &max_supply_ctx.user],
        )
        .is_err(),
        "Prop RFQ buy should enforce max supply"
    );

    let mut max_mint_ctx = setup_prop_rfq();
    let boss = max_mint_ctx.payer.pubkey();
    add_prop_rfq_vector(&mut max_mint_ctx);
    let ix = build_transfer_mint_authority_to_program_ix(
        &boss,
        &max_mint_ctx.rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut max_mint_ctx.svm, &[ix], &[&max_mint_ctx.payer]).unwrap();
    let ix = build_configure_max_mint_amount_ix(&boss, quote.token_out_amount - 1);
    send_tx(&mut max_mint_ctx.svm, &[ix], &[&max_mint_ctx.payer]).unwrap();
    let buy_ix = build_open_swap_buy_ix(
        &max_mint_ctx.rwa_mint,
        &max_mint_ctx.user.pubkey(),
        &boss,
        &max_mint_ctx.usdc_mint,
        &max_mint_ctx.rwa_mint,
        1_000_000,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    assert!(
        send_tx(
            &mut max_mint_ctx.svm,
            &[buy_ix],
            &[&max_mint_ctx.payer, &max_mint_ctx.user],
        )
        .is_err(),
        "Prop RFQ buy should enforce max minted amount per mint"
    );
}

#[test]
fn test_open_swap_buy_rejects_noncanonical_mint_authority() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    add_prop_rfq_vector(&mut ctx);
    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let fake_mint_authority = new_test_keypair();
    ctx.svm
        .airdrop(&fake_mint_authority.pubkey(), INITIAL_LAMPORTS)
        .unwrap();

    let mut ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    ix.accounts[22] = AccountMeta::new_readonly(fake_mint_authority.pubkey(), false);

    let result = send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]);
    assert!(
        result.is_err(),
        "Prop RFQ buy should reject a non-canonical mint authority account"
    );
}

#[test]
fn test_open_swap_buy_rejects_token_in_transfer_fee() {
    let (mut svm, payer, rwa_mint) = setup_initialized();
    let boss = payer.pubkey();
    let usdg_mint = create_mint_2022_with_transfer_fee(&mut svm, &payer, 6, &boss, 500, 1_000_000);

    let ix = build_make_offer_ix(
        &boss,
        &usdg_mint,
        &rwa_mint,
        0,
        false,
        true,
        &TOKEN_2022_PROGRAM_ID,
    );
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let (offer_pda, _) = find_offer_pda(&usdg_mint, &rwa_mint);
    let ix = build_set_main_offer_ix(&boss, &offer_pda);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let ix = build_configure_prop_rfq_ix(&boss, &usdg_mint, &rwa_mint, true, 700, 25_000);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();

    let current_time = get_clock_time(&svm);
    let ix = build_add_offer_vector_ix(
        &boss,
        &usdg_mint,
        &rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();

    let user = new_test_keypair();
    svm.airdrop(&user.pubkey(), 10 * INITIAL_LAMPORTS).unwrap();
    create_token_account_2022(&mut svm, &usdg_mint, &user.pubkey(), 10_000_000);

    let ix = build_open_swap_buy_ix(
        &rwa_mint,
        &user.pubkey(),
        &boss,
        &usdg_mint,
        &rwa_mint,
        1_000_000,
        0,
        &TOKEN_2022_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    let result = send_tx(&mut svm, &[ix], &[&payer, &user]);
    assert!(
        result.is_err(),
        "Prop RFQ buy should reject Token-2022 transfer-fee assets"
    );
}

#[test]
fn test_quote_swap_buy_rejects_token_in_transfer_fee() {
    let (mut svm, payer, rwa_mint) = setup_initialized();
    let boss = payer.pubkey();
    let usdg_mint = create_mint_2022_with_transfer_fee(&mut svm, &payer, 6, &boss, 500, 1_000_000);

    let ix = build_make_offer_ix(
        &boss,
        &usdg_mint,
        &rwa_mint,
        0,
        false,
        true,
        &TOKEN_2022_PROGRAM_ID,
    );
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let (offer_pda, _) = find_offer_pda(&usdg_mint, &rwa_mint);
    let ix = build_set_main_offer_ix(&boss, &offer_pda);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let ix = build_configure_prop_rfq_ix(&boss, &usdg_mint, &rwa_mint, true, 700, 25_000);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();

    let quote_ix = build_quote_swap_ix(&rwa_mint, &usdg_mint, &rwa_mint, 1_000_000);
    let result = send_tx(&mut svm, &[quote_ix], &[&payer]);
    assert!(
        result.is_err(),
        "Prop RFQ buy quote should reject Token-2022 transfer-fee assets"
    );
}

#[test]
fn test_quote_swap_sell_rejects_token_out_transfer_fee() {
    let (mut svm, payer, rwa_mint) = setup_initialized();
    let boss = payer.pubkey();
    let asset_mint = create_mint_2022_with_transfer_fee(&mut svm, &payer, 6, &boss, 500, 1_000_000);

    let ix = build_make_offer_ix(
        &boss,
        &asset_mint,
        &rwa_mint,
        0,
        false,
        true,
        &TOKEN_2022_PROGRAM_ID,
    );
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let (offer_pda, _) = find_offer_pda(&asset_mint, &rwa_mint);
    let ix = build_set_main_offer_ix(&boss, &offer_pda);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();
    let ix = build_configure_prop_rfq_ix(&boss, &asset_mint, &rwa_mint, true, 700, 25_000);
    send_tx(&mut svm, &[ix], &[&payer]).unwrap();

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    let redemption_vault_asset = create_token_account_2022(
        &mut svm,
        &asset_mint,
        &redemption_vault_authority,
        1_000_000,
    );
    let mut quote_ix = build_quote_swap_ix(&rwa_mint, &rwa_mint, &asset_mint, 1_000_000_000);
    quote_ix.accounts[5] = AccountMeta::new_readonly(redemption_vault_asset, false);
    quote_ix.accounts[8] = AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false);

    let result = send_tx(&mut svm, &[quote_ix], &[&payer]);
    assert!(
        result.is_err(),
        "Prop RFQ sell quote should reject Token-2022 transfer-fee payout assets"
    );
}

#[test]
fn test_open_swap_sell_rolls_epoch_tracker_before_recording_trade() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let ix = build_configure_prop_rfq_with_params_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        true,
        700,
        25_000,
        20,
        10_000,
        10,
        20_000,
        0,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    prepare_prop_rfq_sell_side(&mut ctx, 0);
    advance_slot(&mut ctx.svm);
    let ix = build_configure_prop_rfq_with_params_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        true,
        700,
        25_000,
        20,
        10_000,
        10,
        20_000,
        0,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let sell_amount = 100_000_000;
    let sell_ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[sell_ix], &[&ctx.payer, &ctx.user]).unwrap();

    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    let first_state = read_prop_rfq_pair_state(&ctx.svm, &offer_pda);
    assert_eq!(first_state.curr_sell_trade_count, 1);
    assert_eq!(first_state.curr_sell_value_stable, sell_amount / 1_000);

    advance_clock_by(&mut ctx.svm, 11);
    let sell_ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[sell_ix], &[&ctx.payer, &ctx.user]).unwrap();
    let rolled_state = read_prop_rfq_pair_state(&ctx.svm, &offer_pda);

    assert!(rolled_state.epoch_start > first_state.epoch_start);
    assert_eq!(
        rolled_state.prev_net_sell_value_stable,
        first_state.curr_sell_value_stable
    );
    assert_eq!(rolled_state.curr_sell_trade_count, 1);
    assert_eq!(rolled_state.curr_sell_value_stable, sell_amount / 1_000);
}

#[test]
fn test_open_swap_buy_refills_redemption_vault_until_target_then_overflows_to_boss() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_make_redemption_offer_ix(
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix =
        build_update_redemption_offer_vault_target_ix(&boss, &ctx.rwa_mint, &ctx.usdc_mint, 1_500);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (vault_authority, _) = find_offer_vault_authority_pda();
    set_and_refresh_circulating_supply_exclusions(
        &mut ctx.svm,
        &ctx.payer,
        &ctx.rwa_mint,
        &[vault_authority],
    );

    let first_quote_ix =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 1_000_000);
    let first_quote_metadata = send_tx(&mut ctx.svm, &[first_quote_ix], &[&ctx.payer]).unwrap();
    let first_quote = SwapQuote::try_from_slice(get_return_data(&first_quote_metadata)).unwrap();

    let first_buy_ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        first_quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[first_buy_ix], &[&ctx.payer, &ctx.user]).unwrap();

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    let redemption_vault_usdc = derive_ata(
        &redemption_vault_authority,
        &ctx.usdc_mint,
        &TOKEN_PROGRAM_ID,
    );
    let proceeds_usdc =
        get_associated_token_address(&find_prop_rfq_proceeds_vault_pda().0, &ctx.usdc_mint);
    let (offer_pda, _) = find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint);
    assert_eq!(get_token_balance(&ctx.svm, &redemption_vault_usdc), 0);
    assert_eq!(get_token_balance(&ctx.svm, &proceeds_usdc), 1_000_000);
    assert_eq!(
        read_prop_rfq_pair_state(&ctx.svm, &offer_pda).curr_buy_value_stable,
        0,
        "a buy that does not refill the redemption vault must not relieve pressure"
    );

    advance_slot(&mut ctx.svm);
    refresh_circulating_supply_excluded_balance(
        &mut ctx.svm,
        &ctx.payer,
        &ctx.rwa_mint,
        &[vault_authority],
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let second_quote_ix =
        build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 100_000);
    let second_quote_metadata = send_tx(&mut ctx.svm, &[second_quote_ix], &[&ctx.payer]).unwrap();
    let second_quote = SwapQuote::try_from_slice(get_return_data(&second_quote_metadata)).unwrap();

    let second_buy_ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        100_000,
        second_quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[second_buy_ix], &[&ctx.payer, &ctx.user]).unwrap();

    assert_eq!(get_token_balance(&ctx.svm, &redemption_vault_usdc), 100_000);
    assert_eq!(get_token_balance(&ctx.svm, &proceeds_usdc), 1_000_000);
    assert_eq!(
        read_prop_rfq_pair_state(&ctx.svm, &offer_pda).curr_buy_value_stable,
        100_000,
        "a complete refill must relieve pressure by the full refill amount"
    );

    let third_quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.usdc_mint, &ctx.rwa_mint, 100_001);
    let third_quote_metadata = send_tx(&mut ctx.svm, &[third_quote_ix], &[&ctx.payer]).unwrap();
    let third_quote = SwapQuote::try_from_slice(get_return_data(&third_quote_metadata)).unwrap();

    let third_buy_ix = build_open_swap_buy_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        100_001,
        third_quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[third_buy_ix], &[&ctx.payer, &ctx.user]).unwrap();

    assert_eq!(get_token_balance(&ctx.svm, &redemption_vault_usdc), 150_000);
    assert_eq!(get_token_balance(&ctx.svm, &proceeds_usdc), 1_050_001);
    assert_eq!(
        read_prop_rfq_pair_state(&ctx.svm, &offer_pda).curr_buy_value_stable,
        150_000,
        "a partial refill must relieve pressure only by the amount added to the vault"
    );

    let destination = new_test_keypair();
    ctx.svm
        .airdrop(&destination.pubkey(), INITIAL_LAMPORTS)
        .unwrap();
    let (prop_rfq_proceeds_vault_pda, _) = find_prop_rfq_proceeds_vault_pda();
    let ix = build_set_configurable_vault_destination_ix(
        &boss,
        &prop_rfq_proceeds_vault_pda,
        ConfigurableVaultKind::PropRfqProceeds.as_u8(),
        &destination.pubkey(),
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_withdraw_configurable_vault_ix(
        &destination.pubkey(),
        &prop_rfq_proceeds_vault_pda,
        &destination.pubkey(),
        &ctx.usdc_mint,
        ConfigurableVaultKind::PropRfqProceeds.as_u8(),
        0,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&destination]).unwrap();
    assert_eq!(get_token_balance(&ctx.svm, &proceeds_usdc), 0);
    assert_eq!(
        get_token_balance(
            &ctx.svm,
            &get_associated_token_address(&destination.pubkey(), &ctx.usdc_mint),
        ),
        1_050_001
    );
}

#[test]
fn test_quote_and_open_swap_support_sell_side() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    configure_minimum_sell_haircut(&mut ctx, 0);

    let ix = build_make_redemption_offer_ix(
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        500,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_update_redemption_offer_prop_rfq_sell_fee_ix(
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        500,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let sell_amount = 100_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    assert_eq!(quote.offer, find_offer_pda(&ctx.usdc_mint, &ctx.rwa_mint).0);
    assert_eq!(quote.token_in_net_amount, 95_000_000);
    assert_eq!(quote.token_in_fee_amount, 5_000_000);
    assert_eq!(quote.token_out_amount, 95_000);

    let supply_before = get_mint_supply(&ctx.svm, &ctx.rwa_mint);
    let vault_before = get_token_balance(
        &ctx.svm,
        &derive_ata(
            &redemption_vault_authority,
            &ctx.usdc_mint,
            &TOKEN_PROGRAM_ID,
        ),
    );

    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let user_usdc = get_token_balance(
        &ctx.svm,
        &get_associated_token_address(&ctx.user.pubkey(), &ctx.usdc_mint),
    );
    assert_eq!(user_usdc, 10_000_000_000 + quote.token_out_amount);
    assert_eq!(
        get_mint_supply(&ctx.svm, &ctx.rwa_mint),
        supply_before - 95_000_000
    );
    assert_eq!(
        get_token_balance(
            &ctx.svm,
            &derive_ata(
                &redemption_vault_authority,
                &ctx.usdc_mint,
                &TOKEN_PROGRAM_ID
            ),
        ),
        vault_before - quote.token_out_amount
    );
}

#[test]
fn test_open_swap_sell_refreshes_market_stats_before_hard_wall_target() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    configure_minimum_sell_haircut(&mut ctx, 0);

    let ix = build_make_redemption_offer_ix(
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix =
        build_update_redemption_offer_vault_target_ix(&boss, &ctx.rwa_mint, &ctx.usdc_mint, 5_000);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000,
    );

    let (market_stats_pda, _) = find_market_stats_pda();
    assert!(ctx.svm.get_account(&market_stats_pda).is_none());

    let sell_amount = 100_000_000;
    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let market_stats = read_market_stats(&ctx.svm);
    assert_eq!(
        market_stats.circulating_supply,
        get_mint_supply(&ctx.svm, &ctx.rwa_mint)
    );
}

#[test]
fn test_quote_swap_sell_caps_hard_wall_reserve_by_vault_target() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    configure_minimum_sell_haircut(&mut ctx, 0);

    let ix = build_make_redemption_offer_ix(
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let market_stats = read_market_stats(&ctx.svm);
    assert!(market_stats.tvl > 0);

    let sell_amount = 1_000_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let vault_balance_quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    let ix = build_update_redemption_offer_vault_target_ix(&boss, &ctx.rwa_mint, &ctx.usdc_mint, 1);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    advance_slot(&mut ctx.svm);
    let target_reserve = hard_wall_reserve_from_tvl(market_stats.tvl, 1, 6, 9).unwrap();
    assert!(target_reserve < 10_000_000_000);

    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let target_quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    assert_eq!(
        target_quote.token_in_net_amount,
        vault_balance_quote.token_in_net_amount
    );
    assert!(
        target_quote.token_out_amount < vault_balance_quote.token_out_amount,
        "vault target should cap the hard-wall reserve: target_reserve={target_reserve}, vault_quote={}, target_quote={}",
        vault_balance_quote.token_out_amount,
        target_quote.token_out_amount
    );
}

#[test]
fn test_open_swap_sell_accrues_buffer_before_burning_rwa() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (main_asset_mint, main_offer) = configure_main_offer_with_asset_and_apr(
        &mut ctx.svm,
        &ctx.payer,
        &ctx.rwa_mint,
        &TOKEN_PROGRAM_ID,
        50_000,
    );
    let current_time = get_clock_time(&ctx.svm);
    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    configure_minimum_sell_haircut(&mut ctx, 0);

    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000,
    );

    let ix = build_initialize_buffer_ix(&boss, &main_offer, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_set_buffer_gross_yield_ix(&boss, &main_offer, &ctx.rwa_mint, 150_000);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &main_asset_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    advance_clock_by(&mut ctx.svm, ONE_YEAR_SECONDS);

    let buffer_state_before = read_buffer_state(&ctx.svm);
    let supply_before = get_mint_supply(&ctx.svm, &ctx.rwa_mint);
    let buffer_vault = derive_ata(
        &find_reserve_vault_authority_pda().0,
        &ctx.rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    let buffer_vault_before = get_token_balance(&ctx.svm, &buffer_vault);

    let sell_amount = 100_000_000;
    let ix = build_open_swap_sell_ix_with_main_offer(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
        &main_offer,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let expected_buffer_accrual =
        (buffer_state_before.previous_supply as u128 * 100_000 / (1_000_000 + 50_000)) as u64;
    let supply_after = get_mint_supply(&ctx.svm, &ctx.rwa_mint);
    let buffer_state_after = read_buffer_state(&ctx.svm);

    assert_eq!(buffer_state_before.previous_supply, supply_before);
    assert_eq!(
        get_token_balance(&ctx.svm, &buffer_vault) - buffer_vault_before,
        expected_buffer_accrual
    );
    assert_eq!(
        supply_after,
        supply_before + expected_buffer_accrual - sell_amount
    );
    assert_eq!(buffer_state_after.previous_supply, supply_after);
}

#[test]
fn test_open_swap_buy_prices_buffer_from_main_offer() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let (_main_asset_mint, main_offer) = configure_main_offer_with_asset_and_apr(
        &mut ctx.svm,
        &ctx.payer,
        &ctx.rwa_mint,
        &TOKEN_PROGRAM_ID,
        50_000,
    );
    add_prop_rfq_vector(&mut ctx);

    let ix = build_mint_to_ix_for_offer(
        &boss,
        &ctx.rwa_mint,
        1_000_000_000,
        &TOKEN_PROGRAM_ID,
        &main_offer,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_initialize_buffer_ix(&boss, &main_offer, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    let ix = build_set_buffer_gross_yield_ix(&boss, &main_offer, &ctx.rwa_mint, 150_000);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    advance_clock_by(&mut ctx.svm, ONE_YEAR_SECONDS);

    let buffer_state_before = read_buffer_state(&ctx.svm);
    let supply_before = get_mint_supply(&ctx.svm, &ctx.rwa_mint);
    let buffer_vault = derive_ata(
        &find_reserve_vault_authority_pda().0,
        &ctx.rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    let buffer_vault_before = get_token_balance(&ctx.svm, &buffer_vault);

    let ix = build_open_swap_buy_ix_with_main_offer(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        1_000_000,
        0,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
        &main_offer,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let expected_buffer_accrual =
        (buffer_state_before.previous_supply as u128 * 100_000 / (1_000_000 + 50_000)) as u64;
    let user_mint = get_token_balance(
        &ctx.svm,
        &get_associated_token_address(&ctx.user.pubkey(), &ctx.rwa_mint),
    );
    let supply_after = get_mint_supply(&ctx.svm, &ctx.rwa_mint);

    assert_eq!(buffer_state_before.previous_supply, supply_before);
    assert_eq!(
        get_token_balance(&ctx.svm, &buffer_vault) - buffer_vault_before,
        expected_buffer_accrual
    );
    assert_eq!(user_mint, 1_000_000_000);
    assert_eq!(
        supply_after,
        supply_before + expected_buffer_accrual + user_mint
    );
    assert_eq!(read_buffer_state(&ctx.svm).previous_supply, supply_after);
}

#[test]
fn test_open_swap_buy_rejects_invalid_buffer_accounts() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    add_prop_rfq_vector(&mut ctx);
    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    initialize_prop_rfq_buffer(&mut ctx, 100_000);
    advance_clock_by(&mut ctx.svm, ONE_YEAR_SECONDS);

    for (account_index, label) in [
        (23, "buffer state"),
        (24, "reserve vault"),
        (25, "management fee vault"),
        (26, "performance fee vault"),
    ] {
        let previous_supply = read_buffer_state(&ctx.svm).previous_supply;
        let mut ix = build_open_swap_buy_ix(
            &ctx.rwa_mint,
            &ctx.user.pubkey(),
            &boss,
            &ctx.usdc_mint,
            &ctx.rwa_mint,
            1_000_000,
            0,
            &TOKEN_PROGRAM_ID,
            &TOKEN_PROGRAM_ID,
        );
        ix.accounts[account_index] = AccountMeta::new(Pubkey::new_unique(), false);

        assert!(
            send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).is_err(),
            "Prop RFQ buy should reject invalid {label}"
        );
        assert_eq!(read_buffer_state(&ctx.svm).previous_supply, previous_supply);
    }
}

#[test]
fn test_open_swap_sell_rejects_invalid_buffer_accounts() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    prepare_prop_rfq_sell_side(&mut ctx, 0);
    initialize_prop_rfq_buffer(&mut ctx, 100_000);
    advance_clock_by(&mut ctx.svm, ONE_YEAR_SECONDS);

    for (account_index, label) in [
        (19, "buffer state"),
        (20, "reserve vault"),
        (21, "management fee vault"),
        (22, "performance fee vault"),
    ] {
        let previous_supply = read_buffer_state(&ctx.svm).previous_supply;
        let mut ix = build_open_swap_sell_ix(
            &ctx.rwa_mint,
            &ctx.user.pubkey(),
            &boss,
            &ctx.rwa_mint,
            &ctx.usdc_mint,
            100_000_000,
            0,
            &TOKEN_PROGRAM_ID,
            &TOKEN_PROGRAM_ID,
        );
        ix.accounts[account_index] = AccountMeta::new(Pubkey::new_unique(), false);

        assert!(
            send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).is_err(),
            "Prop RFQ sell should reject invalid {label}"
        );
        assert_eq!(read_buffer_state(&ctx.svm).previous_supply, previous_supply);
    }
}

#[test]
fn test_sell_side_uses_zero_fee_when_redemption_offer_is_uninitialized() {
    let mut ctx = setup_prop_rfq();
    let boss = ctx.payer.pubkey();
    let current_time = get_clock_time(&ctx.svm);

    let ix = build_transfer_mint_authority_to_program_ix(&boss, &ctx.rwa_mint, &TOKEN_PROGRAM_ID);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let ix = build_add_offer_vector_ix(
        &boss,
        &ctx.usdc_mint,
        &ctx.rwa_mint,
        Some(current_time),
        current_time,
        1_000_000_000,
        0,
        86_400,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();
    configure_minimum_sell_haircut(&mut ctx, 0);

    let (redemption_vault_authority, _) = find_redemption_vault_authority_pda();
    create_token_account(
        &mut ctx.svm,
        &ctx.usdc_mint,
        &redemption_vault_authority,
        10_000_000_000,
    );
    create_token_account(
        &mut ctx.svm,
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        2_000_000_000,
    );
    let ix = build_refresh_market_stats_ix(&boss, &ctx.usdc_mint, &ctx.rwa_mint);
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer]).unwrap();

    let sell_amount = 100_000_000;
    let quote_ix = build_quote_swap_ix(&ctx.rwa_mint, &ctx.rwa_mint, &ctx.usdc_mint, sell_amount);
    let quote_metadata = send_tx(&mut ctx.svm, &[quote_ix], &[&ctx.payer]).unwrap();
    let quote = SwapQuote::try_from_slice(get_return_data(&quote_metadata)).unwrap();

    assert_eq!(quote.token_in_net_amount, 100_000_000);
    assert_eq!(quote.token_in_fee_amount, 0);
    assert_eq!(quote.token_out_amount, 100_000);

    let supply_before = get_mint_supply(&ctx.svm, &ctx.rwa_mint);
    let vault_before = get_token_balance(
        &ctx.svm,
        &derive_ata(
            &redemption_vault_authority,
            &ctx.usdc_mint,
            &TOKEN_PROGRAM_ID,
        ),
    );

    let ix = build_open_swap_sell_ix(
        &ctx.rwa_mint,
        &ctx.user.pubkey(),
        &boss,
        &ctx.rwa_mint,
        &ctx.usdc_mint,
        sell_amount,
        quote.minimum_out,
        &TOKEN_PROGRAM_ID,
        &TOKEN_PROGRAM_ID,
    );
    send_tx(&mut ctx.svm, &[ix], &[&ctx.payer, &ctx.user]).unwrap();

    let user_usdc = get_token_balance(
        &ctx.svm,
        &get_associated_token_address(&ctx.user.pubkey(), &ctx.usdc_mint),
    );
    assert_eq!(user_usdc, 10_000_000_000 + quote.token_out_amount);
    assert_eq!(
        get_mint_supply(&ctx.svm, &ctx.rwa_mint),
        supply_before - 100_000_000
    );
    assert_eq!(
        get_token_balance(
            &ctx.svm,
            &derive_ata(
                &redemption_vault_authority,
                &ctx.usdc_mint,
                &TOKEN_PROGRAM_ID
            ),
        ),
        vault_before - quote.token_out_amount
    );
}
