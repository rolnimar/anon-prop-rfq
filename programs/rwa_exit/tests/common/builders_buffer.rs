use super::*;

pub fn build_initialize_buffer_ix(boss: &Pubkey, offer: &Pubkey, rwa_mint: &Pubkey) -> Instruction {
    build_initialize_buffer_ix_with_token_program(boss, offer, rwa_mint, &TOKEN_PROGRAM_ID)
}

pub fn build_initialize_buffer_ix_with_token_program(
    boss: &Pubkey,
    offer: &Pubkey,
    rwa_mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (reserve_vault_authority_pda, _) = find_reserve_vault_authority_pda();
    let (management_fee_vault_pda, _) = find_management_fee_vault_pda();
    let (performance_fee_vault_pda, _) = find_performance_fee_vault_pda();
    let buffer_vault_rwa_ata = derive_ata(&reserve_vault_authority_pda, rwa_mint, token_program);
    let management_fee_vault_rwa_ata =
        derive_ata(&management_fee_vault_pda, rwa_mint, token_program);
    let performance_fee_vault_rwa_ata =
        derive_ata(&performance_fee_vault_pda, rwa_mint, token_program);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(state_pda, false),
            AccountMeta::new(buffer_state_pda, false),
            AccountMeta::new(reserve_vault_authority_pda, false),
            AccountMeta::new(management_fee_vault_pda, false),
            AccountMeta::new(performance_fee_vault_pda, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new(*rwa_mint, false),
            AccountMeta::new_readonly(*offer, false),
            AccountMeta::new(buffer_vault_rwa_ata, false),
            AccountMeta::new(management_fee_vault_rwa_ata, false),
            AccountMeta::new(performance_fee_vault_rwa_ata, false),
            AccountMeta::new_readonly(*token_program, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data: ix_discriminator("initialize_buffer").to_vec(),
    }
}

pub fn build_set_main_offer_ix(boss: &Pubkey, offer: &Pubkey) -> Instruction {
    let (state_pda, _) = find_state_pda();
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(state_pda, false),
            AccountMeta::new_readonly(*boss, true),
            AccountMeta::new_readonly(*offer, false),
        ],
        data: ix_discriminator("set_main_offer").to_vec(),
    }
}

pub fn build_settle_buffer_ix(
    worker: &Pubkey,
    main_offer: &Pubkey,
    rwa_mint: &Pubkey,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (mint_authority_pda, _) = find_mint_authority_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (reserve_vault_authority_pda, _) = find_reserve_vault_authority_pda();
    let (management_fee_vault_pda, _) = find_management_fee_vault_pda();
    let (performance_fee_vault_pda, _) = find_performance_fee_vault_pda();
    let (market_stats_pda, _) = find_market_stats_pda();
    let (excluded_balance_pda, _) = find_circulating_supply_excluded_balance_pda();

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new(*worker, true),
            AccountMeta::new(*rwa_mint, false),
            AccountMeta::new_readonly(mint_authority_pda, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(*main_offer, false),
            AccountMeta::new(buffer_state_pda, false),
            AccountMeta::new(
                derive_ata(&reserve_vault_authority_pda, rwa_mint, &TOKEN_PROGRAM_ID),
                false,
            ),
            AccountMeta::new(
                derive_ata(&management_fee_vault_pda, rwa_mint, &TOKEN_PROGRAM_ID),
                false,
            ),
            AccountMeta::new(
                derive_ata(&performance_fee_vault_pda, rwa_mint, &TOKEN_PROGRAM_ID),
                false,
            ),
            AccountMeta::new(market_stats_pda, false),
            AccountMeta::new_readonly(excluded_balance_pda, false),
        ],
        data: ix_discriminator("settle_buffer").to_vec(),
    }
}

pub fn build_set_buffer_gross_yield_ix(
    boss: &Pubkey,
    main_offer: &Pubkey,
    rwa_mint: &Pubkey,
    gross_yield: u64,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (offer_vault_authority_pda, _) = find_offer_vault_authority_pda();
    let (reserve_vault_authority_pda, _) = find_reserve_vault_authority_pda();
    let (management_fee_vault_pda, _) = find_management_fee_vault_pda();
    let (performance_fee_vault_pda, _) = find_performance_fee_vault_pda();
    let (mint_authority_pda, _) = find_mint_authority_pda();
    let (market_stats_pda, _) = find_market_stats_pda();
    let (excluded_balance_pda, _) = find_circulating_supply_excluded_balance_pda();
    let reserve_vault_rwa_ata =
        derive_ata(&reserve_vault_authority_pda, rwa_mint, &TOKEN_PROGRAM_ID);
    let management_fee_vault_rwa_ata =
        derive_ata(&management_fee_vault_pda, rwa_mint, &TOKEN_PROGRAM_ID);
    let performance_fee_vault_rwa_ata =
        derive_ata(&performance_fee_vault_pda, rwa_mint, &TOKEN_PROGRAM_ID);
    let mut data = ix_discriminator("set_buffer_gross_apr").to_vec();
    data.extend_from_slice(&gross_yield.to_le_bytes());
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(*main_offer, false),
            AccountMeta::new(*rwa_mint, false),
            AccountMeta::new_readonly(offer_vault_authority_pda, false),
            AccountMeta::new_readonly(mint_authority_pda, false),
            AccountMeta::new(buffer_state_pda, false),
            AccountMeta::new(reserve_vault_rwa_ata, false),
            AccountMeta::new(management_fee_vault_rwa_ata, false),
            AccountMeta::new(performance_fee_vault_rwa_ata, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(market_stats_pda, false),
            AccountMeta::new_readonly(excluded_balance_pda, false),
        ],
        data,
    }
}

pub fn build_set_buffer_fee_config_ix(
    boss: &Pubkey,
    main_offer: &Pubkey,
    rwa_mint: &Pubkey,
    management_fee_basis_points: u16,
    performance_fee_basis_points: u16,
    performance_fee_high_watermark_enabled: bool,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (offer_vault_authority_pda, _) = find_offer_vault_authority_pda();
    let (mint_authority_pda, _) = find_mint_authority_pda();
    let (market_stats_pda, _) = find_market_stats_pda();
    let (excluded_balance_pda, _) = find_circulating_supply_excluded_balance_pda();
    let reserve_vault_rwa_ata = derive_ata(
        &find_reserve_vault_authority_pda().0,
        rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    let management_fee_vault_rwa_ata = derive_ata(
        &find_management_fee_vault_pda().0,
        rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    let performance_fee_vault_rwa_ata = derive_ata(
        &find_performance_fee_vault_pda().0,
        rwa_mint,
        &TOKEN_PROGRAM_ID,
    );
    let mut data = ix_discriminator("set_buffer_fee_config").to_vec();
    data.extend_from_slice(&management_fee_basis_points.to_le_bytes());
    data.extend_from_slice(&performance_fee_basis_points.to_le_bytes());
    data.push(performance_fee_high_watermark_enabled as u8);
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(*main_offer, false),
            AccountMeta::new(*rwa_mint, false),
            AccountMeta::new_readonly(offer_vault_authority_pda, false),
            AccountMeta::new_readonly(mint_authority_pda, false),
            AccountMeta::new(buffer_state_pda, false),
            AccountMeta::new(reserve_vault_rwa_ata, false),
            AccountMeta::new(management_fee_vault_rwa_ata, false),
            AccountMeta::new(performance_fee_vault_rwa_ata, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(market_stats_pda, false),
            AccountMeta::new_readonly(excluded_balance_pda, false),
        ],
        data,
    }
}

pub fn build_deposit_reserve_vault_ix(
    depositor: &Pubkey,
    rwa_mint: &Pubkey,
    amount: u64,
) -> Instruction {
    build_deposit_reserve_vault_ix_with_token_program(
        depositor,
        rwa_mint,
        amount,
        &TOKEN_PROGRAM_ID,
    )
}

pub fn build_deposit_reserve_vault_ix_with_token_program(
    depositor: &Pubkey,
    rwa_mint: &Pubkey,
    amount: u64,
    token_program: &Pubkey,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (reserve_vault_authority_pda, _) = find_reserve_vault_authority_pda();
    let depositor_rwa_ata = derive_ata(depositor, rwa_mint, token_program);
    let reserve_vault_rwa_ata = derive_ata(&reserve_vault_authority_pda, rwa_mint, token_program);
    let mut data = ix_discriminator("deposit_reserve_vault").to_vec();
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new_readonly(buffer_state_pda, false),
            AccountMeta::new_readonly(reserve_vault_authority_pda, false),
            AccountMeta::new_readonly(*rwa_mint, false),
            AccountMeta::new(depositor_rwa_ata, false),
            AccountMeta::new(reserve_vault_rwa_ata, false),
            AccountMeta::new(*depositor, true),
            AccountMeta::new_readonly(*token_program, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

pub fn build_withdraw_reserve_vault_ix(
    boss: &Pubkey,
    rwa_mint: &Pubkey,
    amount: u64,
) -> Instruction {
    build_withdraw_reserve_vault_ix_with_token_program(boss, rwa_mint, amount, &TOKEN_PROGRAM_ID)
}

pub fn build_withdraw_reserve_vault_ix_with_token_program(
    boss: &Pubkey,
    rwa_mint: &Pubkey,
    amount: u64,
    token_program: &Pubkey,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (reserve_vault_authority_pda, _) = find_reserve_vault_authority_pda();
    let boss_rwa_ata = derive_ata(boss, rwa_mint, token_program);
    let reserve_vault_rwa_ata = derive_ata(&reserve_vault_authority_pda, rwa_mint, token_program);
    let mut data = ix_discriminator("withdraw_reserve_vault").to_vec();
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new_readonly(buffer_state_pda, false),
            AccountMeta::new_readonly(reserve_vault_authority_pda, false),
            AccountMeta::new_readonly(*rwa_mint, false),
            AccountMeta::new(boss_rwa_ata, false),
            AccountMeta::new(reserve_vault_rwa_ata, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(*token_program, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

pub fn build_burn_for_nav_increase_ix(
    boss: &Pubkey,
    main_offer: &Pubkey,
    rwa_mint: &Pubkey,
    asset_adjustment_amount: u64,
) -> Instruction {
    let (state_pda, _) = find_state_pda();
    let (buffer_state_pda, _) = find_buffer_state_pda();
    let (offer_vault_authority_pda, _) = find_offer_vault_authority_pda();
    let (reserve_vault_authority_pda, _) = find_reserve_vault_authority_pda();
    let (management_fee_vault_pda, _) = find_management_fee_vault_pda();
    let (performance_fee_vault_pda, _) = find_performance_fee_vault_pda();
    let (mint_authority_pda, _) = find_mint_authority_pda();
    let (market_stats_pda, _) = find_market_stats_pda();
    let (excluded_balance_pda, _) = find_circulating_supply_excluded_balance_pda();
    let buffer_vault_rwa_ata =
        derive_ata(&reserve_vault_authority_pda, rwa_mint, &TOKEN_PROGRAM_ID);
    let management_fee_vault_rwa_ata =
        derive_ata(&management_fee_vault_pda, rwa_mint, &TOKEN_PROGRAM_ID);
    let performance_fee_vault_rwa_ata =
        derive_ata(&performance_fee_vault_pda, rwa_mint, &TOKEN_PROGRAM_ID);

    let mut data = ix_discriminator("burn_for_nav_increase").to_vec();
    data.extend_from_slice(&asset_adjustment_amount.to_le_bytes());

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new(buffer_state_pda, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(*main_offer, false),
            AccountMeta::new(*rwa_mint, false),
            AccountMeta::new_readonly(offer_vault_authority_pda, false),
            AccountMeta::new_readonly(reserve_vault_authority_pda, false),
            AccountMeta::new(buffer_vault_rwa_ata, false),
            AccountMeta::new_readonly(management_fee_vault_pda, false),
            AccountMeta::new(management_fee_vault_rwa_ata, false),
            AccountMeta::new_readonly(performance_fee_vault_pda, false),
            AccountMeta::new(performance_fee_vault_rwa_ata, false),
            AccountMeta::new_readonly(mint_authority_pda, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(market_stats_pda, false),
            AccountMeta::new_readonly(excluded_balance_pda, false),
        ],
        data,
    }
}
