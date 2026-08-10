use rwa_exit::instructions::prop_rfq::{
    apply_hard_wall_liquidity_factor_at_time, record_prop_rfq_buy, record_prop_rfq_sell,
    roll_prop_rfq_volume_tracker, PropRfqPairState, DEFAULT_CADENCE_THRESHOLD,
    DEFAULT_CADENCE_WAVE_SCALED, DEFAULT_CURVE_EXPONENT_SCALED, DEFAULT_CURVE_PEG_HAIRCUT_BPS,
    DEFAULT_EPOCH_DURATION_SECONDS, DEFAULT_WALL_SENSITIVITY_SCALED,
};
use std::cmp::Ordering;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const INITIAL_LIQUIDITY: u64 = 1_000_000_000;
const START_TIME: i64 = 1_000_000;
const PRIMARY_SMALL_BPS: u16 = 10;
const DUST_AMOUNT: u64 = 1_000;

type EvalResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PropParams {
    haircut_bps: u16,
    exponent_scaled: u32,
    wall_sensitivity_scaled: u32,
    cadence_threshold: u32,
    cadence_wave_scaled: u32,
}

impl PropParams {
    fn reference() -> Self {
        Self {
            haircut_bps: DEFAULT_CURVE_PEG_HAIRCUT_BPS,
            exponent_scaled: DEFAULT_CURVE_EXPONENT_SCALED,
            wall_sensitivity_scaled: DEFAULT_WALL_SENSITIVITY_SCALED,
            cadence_threshold: DEFAULT_CADENCE_THRESHOLD,
            cadence_wave_scaled: DEFAULT_CADENCE_WAVE_SCALED,
        }
    }

    fn endpoint_for(self) -> Self {
        Self {
            wall_sensitivity_scaled: 0,
            cadence_wave_scaled: 0,
            ..self
        }
    }

    fn pressure_only_for(self) -> Self {
        Self {
            cadence_wave_scaled: 0,
            ..self
        }
    }

    fn label(self) -> String {
        format!(
            "h{}_e{}_s{}_k{}_y{}",
            self.haircut_bps,
            self.exponent_scaled,
            self.wall_sensitivity_scaled,
            self.cadence_threshold,
            self.cadence_wave_scaled,
        )
    }
}

#[derive(Clone, Debug)]
enum Mechanism {
    FixedNav,
    StaticHaircut(u16),
    Integrated,
    PropEndpoint,
    PropPressure,
    PropFull,
}

impl Mechanism {
    fn label(&self) -> String {
        match self {
            Self::FixedNav => "fixed_nav".to_string(),
            Self::StaticHaircut(bps) => format!("static_{bps}_bps"),
            Self::Integrated => "integrated_curve".to_string(),
            Self::PropEndpoint => "prop_endpoint".to_string(),
            Self::PropPressure => "prop_pressure".to_string(),
            Self::PropFull => "prop_full".to_string(),
        }
    }

    fn is_prop(&self) -> bool {
        matches!(
            self,
            Self::PropEndpoint | Self::PropPressure | Self::PropFull
        )
    }
}

fn mechanisms() -> Vec<Mechanism> {
    let mut values = vec![Mechanism::FixedNav];
    for haircut in [100, 300, 500, 700, 1_000, 2_000, 3_000] {
        values.push(Mechanism::StaticHaircut(haircut));
    }
    values.extend([
        Mechanism::Integrated,
        Mechanism::PropEndpoint,
        Mechanism::PropPressure,
        Mechanism::PropFull,
    ]);
    values
}

fn prop_mechanisms() -> [Mechanism; 3] {
    [
        Mechanism::PropEndpoint,
        Mechanism::PropPressure,
        Mechanism::PropFull,
    ]
}

#[derive(Clone)]
struct Simulator {
    mechanism: Mechanism,
    initial_liquidity: u64,
    liquidity: u64,
    integrated_position: u64,
    state: PropRfqPairState,
}

#[derive(Clone, Copy)]
struct Outcome {
    executed: bool,
    payout: u64,
    liquidity_before: u64,
    liquidity_after: u64,
}

impl Simulator {
    fn new(mechanism: Mechanism) -> Self {
        let reference = PropParams::reference();
        let mut state = PropRfqPairState {
            enabled: true,
            curve_peg_haircut_bps: reference.haircut_bps,
            curve_exponent_scaled: reference.exponent_scaled,
            cadence_threshold: reference.cadence_threshold,
            cadence_wave_scaled: reference.cadence_wave_scaled,
            epoch_duration_seconds: DEFAULT_EPOCH_DURATION_SECONDS,
            wall_sensitivity_scaled: reference.wall_sensitivity_scaled,
            minimum_sell_haircut_rwa: 0,
            epoch_start: START_TIME,
            ..Default::default()
        };
        match mechanism {
            Mechanism::PropEndpoint => {
                state.wall_sensitivity_scaled = 0;
                state.cadence_wave_scaled = 0;
            }
            Mechanism::PropPressure => state.cadence_wave_scaled = 0,
            _ => {}
        }
        Self {
            mechanism,
            initial_liquidity: INITIAL_LIQUIDITY,
            liquidity: INITIAL_LIQUIDITY,
            integrated_position: 0,
            state,
        }
    }

    fn with_prop_params(params: PropParams) -> Self {
        let mut simulator = Self::new(Mechanism::PropFull);
        simulator.state.curve_peg_haircut_bps = params.haircut_bps;
        simulator.state.curve_exponent_scaled = params.exponent_scaled;
        simulator.state.wall_sensitivity_scaled = params.wall_sensitivity_scaled;
        simulator.state.cadence_threshold = params.cadence_threshold;
        simulator.state.cadence_wave_scaled = params.cadence_wave_scaled;
        simulator
    }

    fn quote(&self, submitted: u64, now: i64) -> EvalResult<Option<u64>> {
        if submitted > self.liquidity || self.liquidity == 0 {
            return Ok(None);
        }
        let payout = match self.mechanism {
            Mechanism::FixedNav => submitted,
            Mechanism::StaticHaircut(haircut_bps) => {
                ((submitted as u128)
                    .saturating_mul((10_000_u16.saturating_sub(haircut_bps)) as u128)
                    / 10_000) as u64
            }
            Mechanism::Integrated => self.integrated_quote(submitted),
            Mechanism::PropEndpoint | Mechanism::PropPressure | Mechanism::PropFull => {
                apply_hard_wall_liquidity_factor_at_time(
                    submitted,
                    self.liquidity,
                    self.liquidity,
                    &self.state,
                    now,
                )
                .map_err(anchor_error)?
            }
        };
        Ok(Some(payout.min(submitted)))
    }

    fn integrated_quote(&self, submitted: u64) -> u64 {
        let x0 = self.integrated_position as f64 / self.initial_liquidity as f64;
        let x1 = self.integrated_position.saturating_add(submitted) as f64
            / self.initial_liquidity as f64;
        let exponent_plus_one = 3.5_f64;
        let haircut_integral = 0.07_f64 / exponent_plus_one
            * (x1.powf(exponent_plus_one) - x0.powf(exponent_plus_one));
        let payout_fraction = (x1 - x0 - haircut_integral).max(0.0);
        (payout_fraction * self.initial_liquidity as f64)
            .floor()
            .min(submitted as f64) as u64
    }

    fn sell(&mut self, submitted: u64, now: i64) -> EvalResult<Outcome> {
        let liquidity_before = self.liquidity;
        let Some(payout) = self.quote(submitted, now)? else {
            return Ok(Outcome {
                executed: false,
                payout: 0,
                liquidity_before,
                liquidity_after: liquidity_before,
            });
        };
        self.liquidity = self.liquidity.saturating_sub(payout);
        match self.mechanism {
            Mechanism::Integrated => {
                self.integrated_position = self.integrated_position.saturating_add(submitted)
            }
            Mechanism::PropEndpoint | Mechanism::PropPressure | Mechanism::PropFull => {
                record_prop_rfq_sell(&mut self.state, submitted, now).map_err(anchor_error)?;
            }
            _ => {}
        }
        Ok(Outcome {
            executed: true,
            payout,
            liquidity_before,
            liquidity_after: self.liquidity,
        })
    }

    fn refill(&mut self, funded: u64, recorded_relief: u64, now: i64) -> EvalResult<()> {
        self.liquidity = self
            .liquidity
            .checked_add(funded)
            .ok_or_else(|| io::Error::other("liquidity overflow"))?;
        if self.mechanism.is_prop() {
            record_prop_rfq_buy(&mut self.state, recorded_relief, now).map_err(anchor_error)?;
        }
        Ok(())
    }
}

fn anchor_error(error: anchor_lang::error::Error) -> Box<dyn Error> {
    Box::new(io::Error::other(format!("{error:?}")))
}

fn bps_amount(bps: u16) -> u64 {
    ((INITIAL_LIQUIDITY as u128) * (bps as u128) / 10_000) as u64
}

fn ratio_bps(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    ((numerator as u128) * 10_000 / denominator as u128) as u64
}

fn signed_ratio_bps(delta: i128, denominator: u64) -> Option<i64> {
    if denominator == 0 {
        return None;
    }
    Some((delta.saturating_mul(10_000) / denominator as i128) as i64)
}

fn optional_i64(value: Option<i64>) -> String {
    value.map_or_else(|| "NA".to_string(), |number| number.to_string())
}

fn push_event(
    events: &mut Vec<String>,
    scenario: &str,
    simulator: &Simulator,
    variant: &str,
    action: &str,
    index: usize,
    now: i64,
    submitted: u64,
    outcome: Outcome,
) {
    events.push(format!(
        "{scenario},{},{variant},{action},{index},{now},{submitted},{},{},{},{},{},{},{},{}",
        simulator.mechanism.label(),
        outcome.payout,
        outcome.executed,
        outcome.liquidity_before,
        outcome.liquidity_after,
        simulator.state.curr_sell_value_stable,
        simulator.state.curr_buy_value_stable,
        simulator.state.prev_net_sell_value_stable,
        simulator.state.curr_sell_trade_count,
    ));
}

fn push_quote_event(
    events: &mut Vec<String>,
    scenario: &str,
    simulator: &Simulator,
    variant: &str,
    index: usize,
    now: i64,
    submitted: u64,
    payout: Option<u64>,
) {
    push_event(
        events,
        scenario,
        simulator,
        variant,
        "quote",
        index,
        now,
        submitted,
        Outcome {
            executed: payout.is_some(),
            payout: payout.unwrap_or(0),
            liquidity_before: simulator.liquidity,
            liquidity_after: simulator.liquidity,
        },
    );
}

fn evaluate_small_after_stress(rows: &mut Vec<String>, events: &mut Vec<String>) -> EvalResult<()> {
    for mechanism in mechanisms() {
        for stress_bps in [1_000, 2_500, 5_000, 9_000] {
            for small_bps in [1, 10, 100] {
                let mut simulator = Simulator::new(mechanism.clone());
                let stress = simulator.sell(bps_amount(stress_bps), START_TIME)?;
                push_event(
                    events,
                    "small_after_stress",
                    &simulator,
                    &format!("stress_{stress_bps}_small_{small_bps}"),
                    "sell",
                    0,
                    START_TIME,
                    bps_amount(stress_bps),
                    stress,
                );
                let small = simulator.sell(bps_amount(small_bps), START_TIME)?;
                push_event(
                    events,
                    "small_after_stress",
                    &simulator,
                    &format!("stress_{stress_bps}_small_{small_bps}"),
                    "sell",
                    1,
                    START_TIME,
                    bps_amount(small_bps),
                    small,
                );
                rows.push(format!(
                    "{},{stress_bps},{small_bps},{},{},{},{},{},{}",
                    mechanism.label(),
                    stress.payout,
                    small.payout,
                    ratio_bps(small.payout, bps_amount(small_bps)),
                    10_000_u64.saturating_sub(ratio_bps(small.payout, bps_amount(small_bps))),
                    ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
                    (!stress.executed) as u8 + (!small.executed) as u8,
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct SplitResult {
    mechanism: String,
    total_bps: u16,
    parts: usize,
    timing: &'static str,
    total_payout: u64,
    one_shot_payout: u64,
    split_gain_bps: Option<i64>,
    reserve_bps: u64,
    failed_parts: usize,
}

fn run_split(
    mechanism: Mechanism,
    total_bps: u16,
    parts: usize,
    timing: &'static str,
    events: &mut Vec<String>,
) -> EvalResult<SplitResult> {
    let total = bps_amount(total_bps);
    let mut one_shot = Simulator::new(mechanism.clone());
    let one_shot_outcome = one_shot.sell(total, START_TIME)?;
    let mut simulator = Simulator::new(mechanism.clone());
    let base = total / parts as u64;
    let remainder = total % parts as u64;
    let first_batch = parts.div_ceil(2);
    let mut total_payout = 0_u64;
    let mut failed_parts = 0_usize;
    for index in 0..parts {
        let submitted = base + u64::from((index as u64) < remainder);
        let now = if timing == "boundary" && parts > 1 {
            if index < first_batch {
                START_TIME + DEFAULT_EPOCH_DURATION_SECONDS - 1
            } else {
                START_TIME + DEFAULT_EPOCH_DURATION_SECONDS
            }
        } else {
            START_TIME
        };
        let outcome = simulator.sell(submitted, now)?;
        total_payout = total_payout.saturating_add(outcome.payout);
        failed_parts += usize::from(!outcome.executed);
        push_event(
            events,
            "order_splitting",
            &simulator,
            &format!("total_{total_bps}_parts_{parts}_{timing}"),
            "sell",
            index,
            now,
            submitted,
            outcome,
        );
    }
    Ok(SplitResult {
        mechanism: mechanism.label(),
        total_bps,
        parts,
        timing,
        total_payout,
        one_shot_payout: one_shot_outcome.payout,
        split_gain_bps: signed_ratio_bps(
            total_payout as i128 - one_shot_outcome.payout as i128,
            one_shot_outcome.payout,
        ),
        reserve_bps: ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
        failed_parts,
    })
}

fn evaluate_splitting(
    rows: &mut Vec<String>,
    events: &mut Vec<String>,
) -> EvalResult<Vec<SplitResult>> {
    let mut results = Vec::new();
    for mechanism in mechanisms() {
        for total_bps in [1_000, 2_500, 5_000, 9_000] {
            for parts in [1, 2, 5, 10, 20, 100] {
                for timing in ["within_epoch", "boundary"] {
                    let result = run_split(mechanism.clone(), total_bps, parts, timing, events)?;
                    rows.push(format!(
                        "{},{},{},{},{},{},{},{},{}",
                        result.mechanism,
                        result.total_bps,
                        result.parts,
                        result.timing,
                        result.total_payout,
                        result.one_shot_payout,
                        optional_i64(result.split_gain_bps),
                        result.reserve_bps,
                        result.failed_parts,
                    ));
                    results.push(result);
                }
            }
        }
    }
    Ok(results)
}

fn evaluate_cadence(rows: &mut Vec<String>, events: &mut Vec<String>) -> EvalResult<()> {
    let victim_raw = bps_amount(PRIMARY_SMALL_BPS);
    for mechanism in prop_mechanisms() {
        let quiet = Simulator::new(mechanism.clone())
            .quote(victim_raw, START_TIME)?
            .unwrap_or(0);
        for (traffic, preliminary_raw) in
            [("dust_grief", DUST_AMOUNT), ("honest_burst", victim_raw)]
        {
            for trade_count in [1, 5, 10, 20, 100] {
                let mut simulator = Simulator::new(mechanism.clone());
                let mut preliminary_payout = 0_u64;
                let mut failed = 0_usize;
                for index in 0..trade_count {
                    let outcome = simulator.sell(preliminary_raw, START_TIME)?;
                    preliminary_payout = preliminary_payout.saturating_add(outcome.payout);
                    failed += usize::from(!outcome.executed);
                    push_event(
                        events,
                        "cadence",
                        &simulator,
                        &format!("{traffic}_{trade_count}"),
                        "sell",
                        index,
                        START_TIME,
                        preliminary_raw,
                        outcome,
                    );
                }
                let victim = simulator.sell(victim_raw, START_TIME)?;
                push_event(
                    events,
                    "cadence",
                    &simulator,
                    &format!("{traffic}_{trade_count}"),
                    "victim_sell",
                    trade_count,
                    START_TIME,
                    victim_raw,
                    victim,
                );
                let submitted = preliminary_raw.saturating_mul(trade_count as u64);
                let preliminary_loss = submitted.saturating_sub(preliminary_payout);
                let victim_harm = quiet.saturating_sub(victim.payout);
                rows.push(format!(
                    "{},{traffic},{trade_count},{preliminary_raw},{},{},{},{},{},{},{}",
                    mechanism.label(),
                    preliminary_loss,
                    quiet,
                    victim.payout,
                    victim_harm,
                    if victim_harm == 0 {
                        "NA".to_string()
                    } else {
                        ratio_bps(preliminary_loss, victim_harm).to_string()
                    },
                    ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
                    failed + usize::from(!victim.executed),
                ));
            }
        }
    }
    Ok(())
}

fn evaluate_buy_relief(rows: &mut Vec<String>, events: &mut Vec<String>) -> EvalResult<()> {
    let stress_raw = bps_amount(2_500);
    let victim_raw = bps_amount(PRIMARY_SMALL_BPS);
    for mechanism in [Mechanism::PropPressure, Mechanism::PropFull] {
        for relief_bps in [0_u16, 2_500, 5_000, 10_000] {
            let nominal_buy = ((stress_raw as u128) * relief_bps as u128 / 10_000) as u64;
            for mode in ["funded_refill", "unfunded_counterfactual"] {
                let mut simulator = Simulator::new(mechanism.clone());
                let stress = simulator.sell(stress_raw, START_TIME)?;
                push_event(
                    events,
                    "buy_relief",
                    &simulator,
                    &format!("{mode}_{relief_bps}"),
                    "stress_sell",
                    0,
                    START_TIME,
                    stress_raw,
                    stress,
                );
                let before_refill = simulator.liquidity;
                let funded = if mode == "funded_refill" {
                    nominal_buy
                } else {
                    0
                };
                simulator.refill(funded, nominal_buy, START_TIME)?;
                push_event(
                    events,
                    "buy_relief",
                    &simulator,
                    &format!("{mode}_{relief_bps}"),
                    "buy_relief",
                    1,
                    START_TIME,
                    nominal_buy,
                    Outcome {
                        executed: true,
                        payout: 0,
                        liquidity_before: before_refill,
                        liquidity_after: simulator.liquidity,
                    },
                );
                let roundtrip = if nominal_buy > 0 {
                    simulator.sell(nominal_buy, START_TIME)?
                } else {
                    Outcome {
                        executed: true,
                        payout: 0,
                        liquidity_before: simulator.liquidity,
                        liquidity_after: simulator.liquidity,
                    }
                };
                push_event(
                    events,
                    "buy_relief",
                    &simulator,
                    &format!("{mode}_{relief_bps}"),
                    "roundtrip_sell",
                    2,
                    START_TIME,
                    nominal_buy,
                    roundtrip,
                );
                let victim_quote = simulator.quote(victim_raw, START_TIME)?;
                push_quote_event(
                    events,
                    "buy_relief",
                    &simulator,
                    &format!("{mode}_{relief_bps}"),
                    3,
                    START_TIME,
                    victim_raw,
                    victim_quote,
                );
                rows.push(format!(
                    "{},{mode},{relief_bps},{},{},{},{},{},{},{},{}",
                    mechanism.label(),
                    funded,
                    nominal_buy,
                    roundtrip.payout,
                    roundtrip.payout as i128 - nominal_buy as i128,
                    victim_quote.unwrap_or(0),
                    ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
                    simulator.state.curr_sell_value_stable,
                    simulator.state.curr_buy_value_stable,
                ));
            }
        }
    }
    Ok(())
}

fn evaluate_ordering(rows: &mut Vec<String>, events: &mut Vec<String>) -> EvalResult<()> {
    let whale_raw = bps_amount(5_000);
    let small_raw = bps_amount(PRIMARY_SMALL_BPS);
    for mechanism in mechanisms() {
        for ordering in ["whale_first", "small_first"] {
            let mut simulator = Simulator::new(mechanism.clone());
            let submitted = if ordering == "whale_first" {
                [whale_raw, small_raw]
            } else {
                [small_raw, whale_raw]
            };
            let first = simulator.sell(submitted[0], START_TIME)?;
            push_event(
                events,
                "ordering",
                &simulator,
                ordering,
                "sell",
                0,
                START_TIME,
                submitted[0],
                first,
            );
            let second = simulator.sell(submitted[1], START_TIME)?;
            push_event(
                events,
                "ordering",
                &simulator,
                ordering,
                "sell",
                1,
                START_TIME,
                submitted[1],
                second,
            );
            let (whale_payout, small_payout) = if ordering == "whale_first" {
                (first.payout, second.payout)
            } else {
                (second.payout, first.payout)
            };
            rows.push(format!(
                "{},{ordering},{whale_payout},{small_payout},{},{}",
                mechanism.label(),
                whale_payout.saturating_add(small_payout),
                ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct RecoveryPoint {
    mechanism: String,
    quote_bps: u16,
    offset_quarters: u8,
    implementation_gap_bps: u64,
    rolled_model_gap_bps: u64,
    parity_gap: i128,
}

fn evaluate_recovery(
    rows: &mut Vec<String>,
    events: &mut Vec<String>,
) -> EvalResult<Vec<RecoveryPoint>> {
    let stress_raw = bps_amount(5_000);
    let mut results = Vec::new();
    for mechanism in prop_mechanisms() {
        let mut simulator = Simulator::new(mechanism.clone());
        let stress = simulator.sell(stress_raw, START_TIME)?;
        push_event(
            events,
            "recovery",
            &simulator,
            "stress_5000",
            "stress_sell",
            0,
            START_TIME,
            stress_raw,
            stress,
        );
        for quote_bps in [PRIMARY_SMALL_BPS, 1_000] {
            let quote_raw = bps_amount(quote_bps);
            let mut cleared = Simulator::new(mechanism.clone());
            cleared.liquidity = simulator.liquidity;
            let cleared_quote = cleared.quote(quote_raw, START_TIME)?.unwrap_or(0);
            for offset_quarters in [0_u8, 1, 2, 4, 5, 6, 8] {
                let now =
                    START_TIME + DEFAULT_EPOCH_DURATION_SECONDS * i64::from(offset_quarters) / 4;
                let implementation_quote = simulator.quote(quote_raw, now)?.unwrap_or(0);
                let mut rolled_model = simulator.clone();
                if offset_quarters >= 4 {
                    roll_prop_rfq_volume_tracker(
                        &mut rolled_model.state,
                        START_TIME + DEFAULT_EPOCH_DURATION_SECONDS,
                    )
                    .map_err(anchor_error)?;
                }
                let rolled_model_quote = rolled_model.quote(quote_raw, now)?.unwrap_or(0);
                let implementation_gap_bps =
                    10_000_u64.saturating_sub(ratio_bps(implementation_quote, cleared_quote));
                let rolled_model_gap_bps =
                    10_000_u64.saturating_sub(ratio_bps(rolled_model_quote, cleared_quote));
                let parity_gap = implementation_quote as i128 - rolled_model_quote as i128;
                push_quote_event(
                    events,
                    "recovery",
                    &simulator,
                    &format!("quote_{quote_bps}_offset_quarters_{offset_quarters}"),
                    offset_quarters as usize,
                    now,
                    quote_raw,
                    Some(implementation_quote),
                );
                rows.push(format!(
                    "{},{quote_bps},{offset_quarters},{now},{cleared_quote},{implementation_quote},{rolled_model_quote},{implementation_gap_bps},{rolled_model_gap_bps},{parity_gap}",
                    mechanism.label(),
                ));
                results.push(RecoveryPoint {
                    mechanism: mechanism.label(),
                    quote_bps,
                    offset_quarters,
                    implementation_gap_bps,
                    rolled_model_gap_bps,
                    parity_gap,
                });
            }
        }
    }
    Ok(results)
}

#[derive(Clone, Copy)]
struct PricingMetrics {
    worst_small_discount_bps: u64,
    max_split_gain_nav_bps: u64,
    max_split_gain_relative_bps: Option<u64>,
    minimum_reserve_bps: u64,
    failed_split_parts: usize,
}

#[derive(Clone, Copy)]
struct CadenceMitigationMetrics {
    aggregate_split_mitigation_bps: Option<i64>,
    worst_workload_split_mitigation_bps: Option<i64>,
    improved_split_workloads: usize,
    unchanged_split_workloads: usize,
    worsened_split_workloads: usize,
}

#[derive(Clone)]
struct ParameterSweepRow {
    params: PropParams,
    pricing: PricingMetrics,
    pressure_only: PricingMetrics,
    endpoint: PricingMetrics,
    cadence_mitigation: CadenceMitigationMetrics,
    grief_pass: bool,
    minimum_grief_cost_harm_ratio_bps: Option<u64>,
    maximum_grief_harm: u64,
    worst_honest_burst_discount_bps: u64,
    maximum_funded_roundtrip_profit: i128,
    recovery_parity_max_abs: u64,
    dominated_by_pressure: bool,
    dominated_by_endpoint: bool,
    dominating_simple_baseline: Option<String>,
    adds_value_over_pressure: bool,
    adds_value_over_endpoint: bool,
    pricing_eligible: bool,
    security_eligible: bool,
    tradeoff_eligible: bool,
}

struct ParameterSweepReport {
    rows: Vec<ParameterSweepRow>,
    frontier: Vec<ParameterSweepRow>,
    ranked: Vec<ParameterSweepRow>,
    security_ranked: Vec<ParameterSweepRow>,
    simple_baselines: Vec<NamedPricingMetrics>,
}

#[derive(Clone)]
struct NamedPricingMetrics {
    mechanism: String,
    pricing: PricingMetrics,
}

fn run_parameter_split(
    params: PropParams,
    total_bps: u16,
    parts: usize,
    timing: &str,
) -> EvalResult<(u64, u64, u64, usize)> {
    let total = bps_amount(total_bps);
    let mut one_shot = Simulator::with_prop_params(params);
    let one_shot_payout = one_shot.sell(total, START_TIME)?.payout;
    let mut simulator = Simulator::with_prop_params(params);
    let base = total / parts as u64;
    let remainder = total % parts as u64;
    let first_batch = parts.div_ceil(2);
    let mut split_payout = 0_u64;
    let mut failed_parts = 0_usize;
    for index in 0..parts {
        let submitted = base + u64::from((index as u64) < remainder);
        let now = if timing == "boundary" && parts > 1 {
            if index < first_batch {
                START_TIME + DEFAULT_EPOCH_DURATION_SECONDS - 1
            } else {
                START_TIME + DEFAULT_EPOCH_DURATION_SECONDS
            }
        } else {
            START_TIME
        };
        let outcome = simulator.sell(submitted, now)?;
        split_payout = split_payout.saturating_add(outcome.payout);
        failed_parts += usize::from(!outcome.executed);
    }
    Ok((
        one_shot_payout,
        split_payout,
        ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
        failed_parts,
    ))
}

fn calculate_pricing_metrics(params: PropParams) -> EvalResult<PricingMetrics> {
    let small_raw = bps_amount(PRIMARY_SMALL_BPS);
    let mut worst_small_discount_bps = 0_u64;
    for stress_bps in [1_000, 2_500, 5_000, 9_000] {
        let mut simulator = Simulator::with_prop_params(params);
        simulator.sell(bps_amount(stress_bps), START_TIME)?;
        let small = simulator.sell(small_raw, START_TIME)?;
        let discount = 10_000_u64.saturating_sub(ratio_bps(small.payout, small_raw));
        worst_small_discount_bps = worst_small_discount_bps.max(discount);
    }

    let mut max_split_gain_nav_bps = 0_u64;
    let mut max_split_gain_relative_bps = Some(0_u64);
    let mut minimum_reserve_bps = 10_000_u64;
    let mut failed_split_parts = 0_usize;
    for total_bps in [1_000, 2_500, 5_000, 9_000] {
        let total = bps_amount(total_bps);
        for parts in [2, 5, 10, 20, 100] {
            for timing in ["within_epoch", "boundary"] {
                let (one_shot, split, reserve_bps, failed) =
                    run_parameter_split(params, total_bps, parts, timing)?;
                let gain = split.saturating_sub(one_shot);
                max_split_gain_nav_bps = max_split_gain_nav_bps.max(ratio_bps(gain, total));
                if gain > 0 && one_shot == 0 {
                    max_split_gain_relative_bps = None;
                } else if let Some(current) = max_split_gain_relative_bps {
                    max_split_gain_relative_bps =
                        Some(current.max(ratio_bps(gain, one_shot.max(1))));
                }
                minimum_reserve_bps = minimum_reserve_bps.min(reserve_bps);
                failed_split_parts = failed_split_parts.max(failed);
            }
        }
    }

    Ok(PricingMetrics {
        worst_small_discount_bps,
        max_split_gain_nav_bps,
        max_split_gain_relative_bps,
        minimum_reserve_bps,
        failed_split_parts,
    })
}

fn run_mechanism_split(
    mechanism: &Mechanism,
    total_bps: u16,
    parts: usize,
    timing: &str,
) -> EvalResult<(u64, u64, u64, usize)> {
    let total = bps_amount(total_bps);
    let mut one_shot = Simulator::new(mechanism.clone());
    let one_shot_payout = one_shot.sell(total, START_TIME)?.payout;
    let mut simulator = Simulator::new(mechanism.clone());
    let base = total / parts as u64;
    let remainder = total % parts as u64;
    let first_batch = parts.div_ceil(2);
    let mut split_payout = 0_u64;
    let mut failed_parts = 0_usize;
    for index in 0..parts {
        let submitted = base + u64::from((index as u64) < remainder);
        let now = if timing == "boundary" && parts > 1 {
            if index < first_batch {
                START_TIME + DEFAULT_EPOCH_DURATION_SECONDS - 1
            } else {
                START_TIME + DEFAULT_EPOCH_DURATION_SECONDS
            }
        } else {
            START_TIME
        };
        let outcome = simulator.sell(submitted, now)?;
        split_payout = split_payout.saturating_add(outcome.payout);
        failed_parts += usize::from(!outcome.executed);
    }
    Ok((
        one_shot_payout,
        split_payout,
        ratio_bps(simulator.liquidity, INITIAL_LIQUIDITY),
        failed_parts,
    ))
}

fn calculate_mechanism_pricing_metrics(mechanism: &Mechanism) -> EvalResult<PricingMetrics> {
    let small_raw = bps_amount(PRIMARY_SMALL_BPS);
    let mut worst_small_discount_bps = 0_u64;
    for stress_bps in [1_000, 2_500, 5_000, 9_000] {
        let mut simulator = Simulator::new(mechanism.clone());
        simulator.sell(bps_amount(stress_bps), START_TIME)?;
        let small = simulator.sell(small_raw, START_TIME)?;
        let discount = 10_000_u64.saturating_sub(ratio_bps(small.payout, small_raw));
        worst_small_discount_bps = worst_small_discount_bps.max(discount);
    }

    let mut max_split_gain_nav_bps = 0_u64;
    let mut max_split_gain_relative_bps = Some(0_u64);
    let mut minimum_reserve_bps = 10_000_u64;
    let mut failed_split_parts = 0_usize;
    for total_bps in [1_000, 2_500, 5_000, 9_000] {
        let total = bps_amount(total_bps);
        for parts in [2, 5, 10, 20, 100] {
            for timing in ["within_epoch", "boundary"] {
                let (one_shot, split, reserve_bps, failed) =
                    run_mechanism_split(mechanism, total_bps, parts, timing)?;
                let gain = split.saturating_sub(one_shot);
                max_split_gain_nav_bps = max_split_gain_nav_bps.max(ratio_bps(gain, total));
                if gain > 0 && one_shot == 0 {
                    max_split_gain_relative_bps = None;
                } else if let Some(current) = max_split_gain_relative_bps {
                    max_split_gain_relative_bps =
                        Some(current.max(ratio_bps(gain, one_shot.max(1))));
                }
                minimum_reserve_bps = minimum_reserve_bps.min(reserve_bps);
                failed_split_parts = failed_split_parts.max(failed);
            }
        }
    }

    Ok(PricingMetrics {
        worst_small_discount_bps,
        max_split_gain_nav_bps,
        max_split_gain_relative_bps,
        minimum_reserve_bps,
        failed_split_parts,
    })
}

fn calculate_simple_baselines() -> EvalResult<Vec<NamedPricingMetrics>> {
    mechanisms()
        .into_iter()
        .filter(|mechanism| !mechanism.is_prop())
        .map(|mechanism| {
            Ok(NamedPricingMetrics {
                mechanism: mechanism.label(),
                pricing: calculate_mechanism_pricing_metrics(&mechanism)?,
            })
        })
        .collect()
}

fn calculate_cadence_mitigation_metrics(
    params: PropParams,
    pressure_only_params: PropParams,
) -> EvalResult<CadenceMitigationMetrics> {
    let mut candidate_gain_sum = 0_u64;
    let mut pressure_gain_sum = 0_u64;
    let mut workload_mitigations = Vec::new();
    let mut improved_split_workloads = 0_usize;
    let mut unchanged_split_workloads = 0_usize;
    let mut worsened_split_workloads = 0_usize;

    for total_bps in [1_000, 2_500, 5_000, 9_000] {
        let total = bps_amount(total_bps);
        for parts in [2, 5, 10, 20, 100] {
            for timing in ["within_epoch", "boundary"] {
                let (candidate_one_shot, candidate_split, _, _) =
                    run_parameter_split(params, total_bps, parts, timing)?;
                let (pressure_one_shot, pressure_split, _, _) =
                    run_parameter_split(pressure_only_params, total_bps, parts, timing)?;
                let candidate_gain =
                    ratio_bps(candidate_split.saturating_sub(candidate_one_shot), total);
                let pressure_gain =
                    ratio_bps(pressure_split.saturating_sub(pressure_one_shot), total);

                candidate_gain_sum = candidate_gain_sum.saturating_add(candidate_gain);
                pressure_gain_sum = pressure_gain_sum.saturating_add(pressure_gain);
                match candidate_gain.cmp(&pressure_gain) {
                    Ordering::Less => improved_split_workloads += 1,
                    Ordering::Equal => unchanged_split_workloads += 1,
                    Ordering::Greater => worsened_split_workloads += 1,
                }
                if pressure_gain > 0 {
                    workload_mitigations.push(
                        ((pressure_gain as i128 - candidate_gain as i128) * 10_000
                            / pressure_gain as i128) as i64,
                    );
                }
            }
        }
    }

    let aggregate_split_mitigation_bps = (pressure_gain_sum > 0).then(|| {
        ((pressure_gain_sum as i128 - candidate_gain_sum as i128) * 10_000
            / pressure_gain_sum as i128) as i64
    });
    let worst_workload_split_mitigation_bps = workload_mitigations.into_iter().min();

    Ok(CadenceMitigationMetrics {
        aggregate_split_mitigation_bps,
        worst_workload_split_mitigation_bps,
        improved_split_workloads,
        unchanged_split_workloads,
        worsened_split_workloads,
    })
}

fn calculate_grief_metrics(params: PropParams) -> EvalResult<(bool, Option<u64>, u64)> {
    let victim_raw = bps_amount(PRIMARY_SMALL_BPS);
    let quiet = Simulator::with_prop_params(params)
        .quote(victim_raw, START_TIME)?
        .unwrap_or(0);
    let mut grief_pass = true;
    let mut minimum_ratio = None;
    let mut maximum_harm = 0_u64;
    for trade_count in [1, 5, 10, 20, 100] {
        let mut simulator = Simulator::with_prop_params(params);
        let mut attacker_payout = 0_u64;
        for _ in 0..trade_count {
            attacker_payout =
                attacker_payout.saturating_add(simulator.sell(DUST_AMOUNT, START_TIME)?.payout);
        }
        let victim_payout = simulator.sell(victim_raw, START_TIME)?.payout;
        let attacker_submitted = DUST_AMOUNT.saturating_mul(trade_count as u64);
        let attacker_loss = attacker_submitted.saturating_sub(attacker_payout);
        let victim_harm = quiet.saturating_sub(victim_payout);
        maximum_harm = maximum_harm.max(victim_harm);
        if victim_harm > 0 {
            let ratio = ratio_bps(attacker_loss, victim_harm);
            minimum_ratio = Some(minimum_ratio.map_or(ratio, |current: u64| current.min(ratio)));
            if attacker_loss <= victim_harm {
                grief_pass = false;
            }
        }
    }
    Ok((grief_pass, minimum_ratio, maximum_harm))
}

fn calculate_honest_burst_discount(params: PropParams) -> EvalResult<u64> {
    let raw = bps_amount(PRIMARY_SMALL_BPS);
    let mut worst_discount = 0_u64;
    for trade_count in [1, 5, 10, 20, 100] {
        let mut simulator = Simulator::with_prop_params(params);
        for _ in 0..trade_count {
            simulator.sell(raw, START_TIME)?;
        }
        let victim = simulator.sell(raw, START_TIME)?;
        worst_discount =
            worst_discount.max(10_000_u64.saturating_sub(ratio_bps(victim.payout, raw)));
    }
    Ok(worst_discount)
}

fn calculate_maximum_funded_roundtrip_profit(params: PropParams) -> EvalResult<i128> {
    let stress_raw = bps_amount(2_500);
    let mut maximum_profit = i128::MIN;
    for relief_bps in [2_500_u16, 5_000, 10_000] {
        let nominal_buy = ((stress_raw as u128) * relief_bps as u128 / 10_000) as u64;
        let mut simulator = Simulator::with_prop_params(params);
        simulator.sell(stress_raw, START_TIME)?;
        simulator.refill(nominal_buy, nominal_buy, START_TIME)?;
        let sell = simulator.sell(nominal_buy, START_TIME)?;
        maximum_profit = maximum_profit.max(sell.payout as i128 - nominal_buy as i128);
    }
    Ok(maximum_profit)
}

fn calculate_recovery_parity_gap(params: PropParams) -> EvalResult<u64> {
    let stress_raw = bps_amount(5_000);
    let diagnostic_raw = bps_amount(1_000);
    let mut simulator = Simulator::with_prop_params(params);
    simulator.sell(stress_raw, START_TIME)?;
    let mut maximum_gap = 0_u64;
    for offset_quarters in [5_i64, 6] {
        let now = START_TIME + DEFAULT_EPOCH_DURATION_SECONDS * offset_quarters / 4;
        let implementation_quote = simulator.quote(diagnostic_raw, now)?.unwrap_or(0);
        let mut rolled_model = simulator.clone();
        roll_prop_rfq_volume_tracker(
            &mut rolled_model.state,
            START_TIME + DEFAULT_EPOCH_DURATION_SECONDS,
        )
        .map_err(anchor_error)?;
        let model_quote = rolled_model.quote(diagnostic_raw, now)?.unwrap_or(0);
        maximum_gap = maximum_gap
            .max((implementation_quote as i128 - model_quote as i128).unsigned_abs() as u64);
    }
    Ok(maximum_gap)
}

fn pricing_dominates(candidate: PricingMetrics, baseline: PricingMetrics) -> bool {
    let no_worse = baseline.worst_small_discount_bps <= candidate.worst_small_discount_bps
        && baseline.max_split_gain_nav_bps <= candidate.max_split_gain_nav_bps
        && baseline.minimum_reserve_bps >= candidate.minimum_reserve_bps;
    let strictly_better = baseline.worst_small_discount_bps < candidate.worst_small_discount_bps
        || baseline.max_split_gain_nav_bps < candidate.max_split_gain_nav_bps
        || baseline.minimum_reserve_bps > candidate.minimum_reserve_bps;
    no_worse && strictly_better
}

fn calculate_parameter_row(
    params: PropParams,
    simple_baselines: &[NamedPricingMetrics],
) -> EvalResult<ParameterSweepRow> {
    let pricing = calculate_pricing_metrics(params)?;
    let pressure_only_params = params.pressure_only_for();
    let pressure_only = calculate_pricing_metrics(pressure_only_params)?;
    let endpoint = calculate_pricing_metrics(params.endpoint_for())?;
    let cadence_mitigation = calculate_cadence_mitigation_metrics(params, pressure_only_params)?;
    let (grief_pass, minimum_grief_cost_harm_ratio_bps, maximum_grief_harm) =
        calculate_grief_metrics(params)?;
    let worst_honest_burst_discount_bps = calculate_honest_burst_discount(params)?;
    let maximum_funded_roundtrip_profit = calculate_maximum_funded_roundtrip_profit(params)?;
    let recovery_parity_max_abs = calculate_recovery_parity_gap(params)?;
    let dominated_by_pressure = pricing_dominates(pricing, pressure_only);
    let dominated_by_endpoint = pricing_dominates(pricing, endpoint);
    let dominating_simple_baseline = simple_baselines
        .iter()
        .find(|baseline| pricing_dominates(pricing, baseline.pricing))
        .map(|baseline| baseline.mechanism.clone());
    let adds_value_over_pressure = !dominated_by_pressure
        && (cadence_mitigation
            .aggregate_split_mitigation_bps
            .unwrap_or(0)
            > 0
            || pricing.minimum_reserve_bps > pressure_only.minimum_reserve_bps);
    let adds_value_over_endpoint = !dominated_by_endpoint
        && (pricing.max_split_gain_nav_bps < endpoint.max_split_gain_nav_bps
            || pricing.minimum_reserve_bps > endpoint.minimum_reserve_bps);
    let pricing_eligible = maximum_funded_roundtrip_profit <= 0 && pricing.failed_split_parts == 0;
    let security_eligible = pricing_eligible && grief_pass;
    let cadence_value_gate = params.cadence_wave_scaled == 0 || adds_value_over_pressure;
    let tradeoff_eligible = pricing_eligible
        && recovery_parity_max_abs == 0
        && dominating_simple_baseline.is_none()
        && adds_value_over_endpoint
        && cadence_value_gate;
    Ok(ParameterSweepRow {
        params,
        pricing,
        pressure_only,
        endpoint,
        cadence_mitigation,
        grief_pass,
        minimum_grief_cost_harm_ratio_bps,
        maximum_grief_harm,
        worst_honest_burst_discount_bps,
        maximum_funded_roundtrip_profit,
        recovery_parity_max_abs,
        dominated_by_pressure,
        dominated_by_endpoint,
        dominating_simple_baseline,
        adds_value_over_pressure,
        adds_value_over_endpoint,
        pricing_eligible,
        security_eligible,
        tradeoff_eligible,
    })
}

fn compare_parameter_rows(left: &ParameterSweepRow, right: &ParameterSweepRow) -> Ordering {
    right
        .cadence_mitigation
        .aggregate_split_mitigation_bps
        .unwrap_or(i64::MIN)
        .cmp(
            &left
                .cadence_mitigation
                .aggregate_split_mitigation_bps
                .unwrap_or(i64::MIN),
        )
        .then_with(|| {
            right
                .pricing
                .minimum_reserve_bps
                .cmp(&left.pricing.minimum_reserve_bps)
        })
        .then_with(|| {
            left.pricing
                .max_split_gain_nav_bps
                .cmp(&right.pricing.max_split_gain_nav_bps)
        })
        .then_with(|| {
            left.pricing
                .worst_small_discount_bps
                .cmp(&right.pricing.worst_small_discount_bps)
        })
        .then_with(|| {
            left.pricing
                .max_split_gain_relative_bps
                .unwrap_or(u64::MAX)
                .cmp(
                    &right
                        .pricing
                        .max_split_gain_relative_bps
                        .unwrap_or(u64::MAX),
                )
        })
        .then_with(|| left.params.label().cmp(&right.params.label()))
}

fn parameter_row_dominates(left: &ParameterSweepRow, right: &ParameterSweepRow) -> bool {
    let no_worse = left.pricing.worst_small_discount_bps <= right.pricing.worst_small_discount_bps
        && left
            .cadence_mitigation
            .aggregate_split_mitigation_bps
            .unwrap_or(i64::MIN)
            >= right
                .cadence_mitigation
                .aggregate_split_mitigation_bps
                .unwrap_or(i64::MIN)
        && left.pricing.max_split_gain_nav_bps <= right.pricing.max_split_gain_nav_bps
        && left.pricing.minimum_reserve_bps >= right.pricing.minimum_reserve_bps;
    let strictly_better = left.pricing.worst_small_discount_bps
        < right.pricing.worst_small_discount_bps
        || left
            .cadence_mitigation
            .aggregate_split_mitigation_bps
            .unwrap_or(i64::MIN)
            > right
                .cadence_mitigation
                .aggregate_split_mitigation_bps
                .unwrap_or(i64::MIN)
        || left.pricing.max_split_gain_nav_bps < right.pricing.max_split_gain_nav_bps
        || left.pricing.minimum_reserve_bps > right.pricing.minimum_reserve_bps;
    no_worse && strictly_better
}

fn evaluate_parameter_sweep() -> EvalResult<ParameterSweepReport> {
    let simple_baselines = calculate_simple_baselines()?;
    let mut rows = Vec::with_capacity(15_120);
    for haircut_bps in [
        100, 300, 500, 700, 1_000, 1_500, 2_000, 3_000, 5_000, 10_000,
    ] {
        for exponent_scaled in [
            10_000, 15_000, 20_000, 25_000, 30_000, 40_000, 50_000, 70_000, 100_000,
        ] {
            for wall_sensitivity_scaled in [2_500, 5_000, 10_000, 20_000, 40_000, 80_000, 160_000] {
                for cadence_threshold in [5, 10, 20, 40] {
                    for cadence_wave_scaled in [0, 5_000, 10_000, 20_000, 30_000, 50_000] {
                        rows.push(calculate_parameter_row(
                            PropParams {
                                haircut_bps,
                                exponent_scaled,
                                wall_sensitivity_scaled,
                                cadence_threshold,
                                cadence_wave_scaled,
                            },
                            &simple_baselines,
                        )?);
                    }
                }
            }
        }
    }
    let eligible: Vec<_> = rows
        .iter()
        .filter(|row| row.pricing_eligible)
        .cloned()
        .collect();
    let tradeoff_eligible: Vec<_> = rows
        .iter()
        .filter(|row| row.tradeoff_eligible)
        .cloned()
        .collect();
    let mut frontier = tradeoff_eligible
        .iter()
        .filter(|candidate| {
            !tradeoff_eligible
                .iter()
                .any(|other| parameter_row_dominates(other, candidate))
        })
        .cloned()
        .collect::<Vec<_>>();
    frontier.sort_by(compare_parameter_rows);
    let mut ranked = eligible;
    ranked.sort_by(compare_parameter_rows);
    let mut security_ranked = rows
        .iter()
        .filter(|row| row.security_eligible)
        .cloned()
        .collect::<Vec<_>>();
    security_ranked.sort_by(compare_parameter_rows);
    Ok(ParameterSweepReport {
        rows,
        frontier,
        ranked,
        security_ranked,
        simple_baselines,
    })
}

fn parameter_sweep_header() -> &'static str {
    "rank,parameter_id,haircut_bps,exponent_scaled,wall_sensitivity_scaled,cadence_threshold,cadence_wave_scaled,worst_small_discount_bps,max_split_gain_nav_bps,max_split_gain_relative_bps,minimum_reserve_bps,failed_split_parts,pressure_split_gain_nav_bps,pressure_minimum_reserve_bps,aggregate_split_mitigation_bps,worst_workload_split_mitigation_bps,improved_split_workloads,unchanged_split_workloads,worsened_split_workloads,grief_pass,minimum_grief_cost_harm_ratio_bps,maximum_grief_harm,worst_honest_burst_discount_bps,maximum_funded_roundtrip_profit,recovery_parity_max_abs,endpoint_small_discount_bps,endpoint_split_gain_nav_bps,endpoint_minimum_reserve_bps,dominated_by_pressure,dominated_by_endpoint,dominating_simple_baseline,adds_value_over_pressure,adds_value_over_endpoint,pricing_eligible,security_eligible,tradeoff_eligible"
}

fn parameter_sweep_row(rank: Option<usize>, row: &ParameterSweepRow) -> String {
    format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        rank.map_or_else(|| "NA".to_string(), |value| value.to_string()),
        row.params.label(),
        row.params.haircut_bps,
        row.params.exponent_scaled,
        row.params.wall_sensitivity_scaled,
        row.params.cadence_threshold,
        row.params.cadence_wave_scaled,
        row.pricing.worst_small_discount_bps,
        row.pricing.max_split_gain_nav_bps,
        row.pricing
            .max_split_gain_relative_bps
            .map_or_else(|| "NA".to_string(), |value| value.to_string()),
        row.pricing.minimum_reserve_bps,
        row.pricing.failed_split_parts,
        row.pressure_only.max_split_gain_nav_bps,
        row.pressure_only.minimum_reserve_bps,
        row.cadence_mitigation
            .aggregate_split_mitigation_bps
            .map_or_else(|| "NA".to_string(), |value| value.to_string()),
        row.cadence_mitigation
            .worst_workload_split_mitigation_bps
            .map_or_else(|| "NA".to_string(), |value| value.to_string()),
        row.cadence_mitigation.improved_split_workloads,
        row.cadence_mitigation.unchanged_split_workloads,
        row.cadence_mitigation.worsened_split_workloads,
        row.grief_pass,
        row.minimum_grief_cost_harm_ratio_bps
            .map_or_else(|| "NA".to_string(), |value| value.to_string()),
        row.maximum_grief_harm,
        row.worst_honest_burst_discount_bps,
        row.maximum_funded_roundtrip_profit,
        row.recovery_parity_max_abs,
        row.endpoint.worst_small_discount_bps,
        row.endpoint.max_split_gain_nav_bps,
        row.endpoint.minimum_reserve_bps,
        row.dominated_by_pressure,
        row.dominated_by_endpoint,
        row.dominating_simple_baseline
            .as_deref()
            .unwrap_or("none"),
        row.adds_value_over_pressure,
        row.adds_value_over_endpoint,
        row.pricing_eligible,
        row.security_eligible,
        row.tradeoff_eligible,
    )
}

fn write_baseline_summary(output_dir: &Path, report: &ParameterSweepReport) -> EvalResult<()> {
    let mut baselines = report.simple_baselines.clone();
    for (mechanism, params) in [
        ("prop_endpoint", PropParams::reference().endpoint_for()),
        ("prop_pressure", PropParams::reference().pressure_only_for()),
        ("prop_full", PropParams::reference()),
    ] {
        baselines.push(NamedPricingMetrics {
            mechanism: mechanism.to_string(),
            pricing: calculate_pricing_metrics(params)?,
        });
    }
    let rows = baselines
        .iter()
        .map(|baseline| {
            format!(
                "{},{},{},{},{},{}",
                baseline.mechanism,
                baseline.pricing.worst_small_discount_bps,
                baseline.pricing.max_split_gain_nav_bps,
                baseline
                    .pricing
                    .max_split_gain_relative_bps
                    .map_or_else(|| "NA".to_string(), |value| value.to_string()),
                baseline.pricing.minimum_reserve_bps,
                baseline.pricing.failed_split_parts,
            )
        })
        .collect::<Vec<_>>();
    write_csv(
        &output_dir.join("baseline_summary.csv"),
        "mechanism,worst_protected_order_discount_bps,max_split_gain_nav_bps,max_split_gain_relative_bps,minimum_reserve_remaining_bps,failed_split_parts",
        &rows,
    )
}

fn write_parameter_sweep(output_dir: &Path, report: &ParameterSweepReport) -> EvalResult<()> {
    let all_rows = report
        .rows
        .iter()
        .map(|row| parameter_sweep_row(None, row))
        .collect::<Vec<_>>();
    write_csv(
        &output_dir.join("parameter_sweep.csv"),
        parameter_sweep_header(),
        &all_rows,
    )?;
    let ranked_rows = report
        .ranked
        .iter()
        .enumerate()
        .map(|(index, row)| parameter_sweep_row(Some(index + 1), row))
        .collect::<Vec<_>>();
    write_csv(
        &output_dir.join("parameter_ranked.csv"),
        parameter_sweep_header(),
        &ranked_rows,
    )?;
    let security_ranked_rows = report
        .security_ranked
        .iter()
        .enumerate()
        .map(|(index, row)| parameter_sweep_row(Some(index + 1), row))
        .collect::<Vec<_>>();
    write_csv(
        &output_dir.join("parameter_security_ranked.csv"),
        parameter_sweep_header(),
        &security_ranked_rows,
    )?;
    let frontier_rows = report
        .frontier
        .iter()
        .enumerate()
        .map(|(index, row)| parameter_sweep_row(Some(index + 1), row))
        .collect::<Vec<_>>();
    write_csv(
        &output_dir.join("parameter_frontier.csv"),
        parameter_sweep_header(),
        &frontier_rows,
    )?;
    Ok(())
}

fn write_parameter_sweep_svg(output_dir: &Path, report: &ParameterSweepReport) -> EvalResult<()> {
    let eligible: Vec<_> = report
        .rows
        .iter()
        .filter(|row| row.pricing_eligible)
        .collect();
    let minimum_mitigation = eligible
        .iter()
        .filter_map(|row| row.cadence_mitigation.aggregate_split_mitigation_bps)
        .min()
        .unwrap_or(0);
    let maximum_mitigation = eligible
        .iter()
        .filter_map(|row| row.cadence_mitigation.aggregate_split_mitigation_bps)
        .max()
        .unwrap_or(0);
    let maximum_small_discount = eligible
        .iter()
        .map(|row| row.pricing.worst_small_discount_bps)
        .max()
        .unwrap_or(1)
        .max(1);
    let mitigation_span = (maximum_mitigation - minimum_mitigation).max(1) as f64;
    let mitigation_y = |mitigation: i64| {
        560.0 - 510.0 * ((mitigation - minimum_mitigation) as f64 / mitigation_span)
    };
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1000\" height=\"650\" viewBox=\"0 0 1000 650\">\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n<text x=\"90\" y=\"30\" font-family=\"sans-serif\" font-size=\"18\">Prop RFQ cadence-mitigation sweep</text>\n<line x1=\"90\" y1=\"560\" x2=\"900\" y2=\"560\" stroke=\"black\"/>\n<line x1=\"90\" y1=\"560\" x2=\"90\" y2=\"50\" stroke=\"black\"/>\n<text x=\"495\" y=\"625\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"16\">worst protected-order discount (bps)</text>\n<text x=\"25\" y=\"305\" transform=\"rotate(-90 25 305)\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"16\">aggregate split mitigation vs pressure-only (bps)</text>\n",
    );
    if minimum_mitigation <= 0 && maximum_mitigation >= 0 {
        let zero_y = mitigation_y(0);
        svg.push_str(&format!(
            "<line x1=\"90\" y1=\"{zero_y:.2}\" x2=\"900\" y2=\"{zero_y:.2}\" stroke=\"#777777\" stroke-dasharray=\"4 4\"/>\n",
        ));
    }
    for row in eligible {
        let x = 90.0
            + 810.0 * (row.pricing.worst_small_discount_bps as f64 / maximum_small_discount as f64);
        let y = mitigation_y(
            row.cadence_mitigation
                .aggregate_split_mitigation_bps
                .unwrap_or(minimum_mitigation),
        );
        let color = if !row.grief_pass {
            "#d62728"
        } else if row.tradeoff_eligible {
            "#2ca02c"
        } else if row.dominated_by_pressure {
            "#999999"
        } else if row.recovery_parity_max_abs > 0 {
            "#ff7f0e"
        } else {
            "#1f77b4"
        };
        svg.push_str(&format!(
            "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"1.8\" fill=\"{color}\" fill-opacity=\"0.45\"><title>{}: mitigation={}, small={}, split={}, reserve={}, grief_pass={}, parity_gap={}</title></circle>\n",
            row.params.label(),
            row.cadence_mitigation
                .aggregate_split_mitigation_bps
                .map_or_else(|| "NA".to_string(), |value| value.to_string()),
            row.pricing.worst_small_discount_bps,
            row.pricing.max_split_gain_nav_bps,
            row.pricing.minimum_reserve_bps,
            row.grief_pass,
            row.recovery_parity_max_abs,
        ));
    }
    for row in &report.frontier {
        let x = 90.0
            + 810.0 * (row.pricing.worst_small_discount_bps as f64 / maximum_small_discount as f64);
        let y = mitigation_y(
            row.cadence_mitigation
                .aggregate_split_mitigation_bps
                .unwrap_or(minimum_mitigation),
        );
        svg.push_str(&format!(
            "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"4\" fill=\"none\" stroke=\"#000000\"><title>frontier {}</title></circle>\n",
            row.params.label(),
        ));
    }
    svg.push_str("<text x=\"500\" y=\"45\" font-family=\"sans-serif\" font-size=\"12\">red: grief failure; green: trade-off eligible; gray: dominated; black ring: frontier</text>\n</svg>\n");
    fs::write(output_dir.join("parameter_sweep.svg"), svg)?;
    Ok(())
}

fn write_csv(path: &Path, header: &str, rows: &[String]) -> EvalResult<()> {
    let mut contents = String::with_capacity(rows.iter().map(String::len).sum::<usize>() + 512);
    contents.push_str(header);
    contents.push('\n');
    for row in rows {
        contents.push_str(row);
        contents.push('\n');
    }
    fs::write(path, contents)?;
    Ok(())
}

fn git_output(arguments: &[&str]) -> String {
    Command::new("git")
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn git_diff_hash() -> String {
    let Ok(diff) = Command::new("git").args(["diff", "HEAD"]).output() else {
        return "unknown".to_string();
    };
    let Ok(mut child) = Command::new("git")
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return "unknown".to_string();
    };
    if let Some(stdin) = &mut child.stdin {
        use std::io::Write;
        if stdin.write_all(&diff.stdout).is_err() {
            return "unknown".to_string();
        }
    }
    child
        .wait_with_output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn write_metadata(output_dir: &Path) -> EvalResult<()> {
    let revision = git_output(&["rev-parse", "HEAD"]);
    let status = git_output(&["status", "--porcelain"]);
    let metadata = format!(
        concat!(
            "{{\n",
            "  \"protocol\": \"evaluation protocol revision 9\",\n",
            "  \"implementation_commit\": \"{}\",\n",
            "  \"tracked_diff_hash\": \"{}\",\n",
            "  \"working_tree_dirty\": {},\n",
            "  \"working_tree_status\": \"{}\",\n",
            "  \"randomness\": \"none\",\n",
            "  \"initial_liquidity\": {},\n",
            "  \"curve_peg_haircut_bps\": {},\n",
            "  \"curve_exponent_scaled\": {},\n",
            "  \"wall_sensitivity_scaled\": {},\n",
            "  \"cadence_threshold\": {},\n",
            "  \"cadence_wave_scaled\": {},\n",
            "  \"epoch_duration_seconds\": {}\n",
            "}}\n"
        ),
        json_escape(&revision),
        git_diff_hash(),
        !status.is_empty(),
        json_escape(&status),
        INITIAL_LIQUIDITY,
        DEFAULT_CURVE_PEG_HAIRCUT_BPS,
        DEFAULT_CURVE_EXPONENT_SCALED,
        DEFAULT_WALL_SENSITIVITY_SCALED,
        DEFAULT_CADENCE_THRESHOLD,
        DEFAULT_CADENCE_WAVE_SCALED,
        DEFAULT_EPOCH_DURATION_SECONDS,
    );
    fs::write(output_dir.join("metadata.json"), metadata)?;
    Ok(())
}

fn write_evaluation_contract(output_dir: &Path) -> EvalResult<()> {
    let contract = r#"{
  "protocol_revision": 9,
  "parameter_grid": {
    "haircut_bps": [100, 300, 500, 700, 1000, 1500, 2000, 3000, 5000, 10000],
    "exponent_scaled_1e4": [10000, 15000, 20000, 25000, 30000, 40000, 50000, 70000, 100000],
    "wall_sensitivity_scaled_1e4": [2500, 5000, 10000, 20000, 40000, 80000, 160000],
    "cadence_threshold": [5, 10, 20, 40],
    "cadence_wave_scaled_1e4": [0, 5000, 10000, 20000, 30000, 50000],
    "cartesian_product_count": 15120
  },
  "workloads": {
    "small_after_stress": {
      "stress_exit_bps": [1000, 2500, 5000, 9000],
      "following_exit_bps": [1, 10, 100]
    },
    "splitting": {
      "total_exit_bps": [1000, 2500, 5000, 9000],
      "parts": [1, 2, 5, 10, 20, 100],
      "timing": ["within_epoch", "across_boundary"],
      "aggregate_mitigation_parts": [2, 5, 10, 20, 100],
      "aggregate_mitigation_workloads": 40,
      "aggregate_mitigation_weighting": "equal sum over each declared workload"
    },
    "cadence": {
      "preliminary_trade_count": [1, 5, 10, 20, 100],
      "dust_trade_base_units": 1000,
      "honest_burst_trade_bps": 10
    },
    "buy_relief_bps_of_prior_sell": [0, 2500, 5000, 10000],
    "ordering": ["stress_then_small", "small_then_stress"],
    "recovery_offset_quarters": [0, 1, 2, 4, 5, 6, 8]
  },
  "integrated_curve_baseline": {
    "marginal_payout": "1 - 0.07*x^2.5",
    "order_payout": "floor(L0*((x1-x0) - (0.07/3.5)*(x1^3.5-x0^3.5)))",
    "x0": "cumulative submitted value before the order divided by L0",
    "x1": "cumulative submitted value after the order divided by L0"
  },
  "gate_order": [
    "pricing feasibility: no profitable funded round trip and no failed split part",
    "exact recovery parity",
    "not dominated by a declared simple baseline",
    "adds value over endpoint pricing and, when cadence is nonzero, pressure-only pricing",
    "four-dimensional non-dominated frontier"
  ],
  "security_diagnostic": "every dust trace causes zero harm or attacker pricing loss exceeds victim harm",
  "interpretation": "comparative characterization only"
}
"#;
    fs::write(output_dir.join("evaluation_contract.json"), contract)?;
    Ok(())
}

fn svg_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_pareto_svg(output_dir: &Path, split_results: &[SplitResult]) -> EvalResult<()> {
    let mut points = Vec::new();
    for mechanism in mechanisms() {
        let mut simulator = Simulator::new(mechanism.clone());
        simulator.sell(bps_amount(5_000), START_TIME)?;
        let small = simulator.sell(bps_amount(PRIMARY_SMALL_BPS), START_TIME)?;
        let discount_bps =
            10_000_u64.saturating_sub(ratio_bps(small.payout, bps_amount(PRIMARY_SMALL_BPS)));
        let split = split_results.iter().find(|result| {
            result.mechanism == mechanism.label()
                && result.total_bps == 5_000
                && result.parts == 20
                && result.timing == "within_epoch"
        });
        if let Some(split) = split {
            points.push((
                mechanism.label(),
                discount_bps,
                split.split_gain_bps.unwrap_or(0).max(0) as u64,
                split.reserve_bps,
            ));
        }
    }
    let max_x = points.iter().map(|point| point.1).max().unwrap_or(1).max(1);
    let max_y = points.iter().map(|point| point.2).max().unwrap_or(1).max(1);
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1100\" height=\"650\" viewBox=\"0 0 1100 650\">\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n<text x=\"90\" y=\"30\" font-family=\"sans-serif\" font-size=\"18\">50% exit: small-order price vs 20-part split gain</text>\n<line x1=\"90\" y1=\"560\" x2=\"700\" y2=\"560\" stroke=\"black\"/>\n<line x1=\"90\" y1=\"560\" x2=\"90\" y2=\"50\" stroke=\"black\"/>\n<text x=\"395\" y=\"625\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"16\">protected-order discount (bps; lower is better)</text>\n<text x=\"25\" y=\"305\" transform=\"rotate(-90 25 305)\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"16\">20-part split gain (bps; lower is better)</text>\n",
    );
    for step in 0..=2 {
        let x = 90.0 + 610.0 * (step as f64 / 2.0);
        let value = max_x * step / 2;
        svg.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"560\" x2=\"{x:.1}\" y2=\"566\" stroke=\"black\"/><text x=\"{x:.1}\" y=\"585\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"12\">{value}</text>\n"
        ));
        let y = 560.0 - 510.0 * (step as f64 / 2.0);
        let value = max_y * step / 2;
        svg.push_str(&format!(
            "<line x1=\"84\" y1=\"{y:.1}\" x2=\"90\" y2=\"{y:.1}\" stroke=\"black\"/><text x=\"78\" y=\"{:.1}\" text-anchor=\"end\" font-family=\"sans-serif\" font-size=\"12\">{value}</text>\n",
            y + 4.0,
        ));
    }
    for (index, (label, discount, split_gain, reserve)) in points.iter().enumerate() {
        let x = 90.0 + 610.0 * (*discount as f64 / max_x as f64);
        let y = 560.0 - 490.0 * (*split_gain as f64 / max_y as f64);
        let radius = 5.0 + 12.0 * (*reserve as f64 / 10_000.0);
        let color = if label == "prop_full" {
            "#d62728"
        } else if label.starts_with("prop_") {
            "#1f77b4"
        } else if label == "integrated_curve" {
            "#2ca02c"
        } else {
            "#777777"
        };
        svg.push_str(&format!(
            "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{radius:.1}\" fill=\"{color}\" fill-opacity=\"0.65\" stroke=\"{color}\"><title>{}: discount={} bps, split gain={} bps, reserve={} bps</title></circle>\n<circle cx=\"750\" cy=\"{}\" r=\"6\" fill=\"{color}\"/><text x=\"765\" y=\"{}\" font-family=\"sans-serif\" font-size=\"12\">{}: d={}, g={}, r={}</text>\n",
            svg_escape(label),
            discount,
            split_gain,
            reserve,
            65 + index * 25,
            69 + index * 25,
            svg_escape(label),
            discount,
            split_gain,
            reserve,
        ));
    }
    svg.push_str("<text x=\"750\" y=\"45\" font-family=\"sans-serif\" font-size=\"13\">legend: d=discount, g=split gain, r=reserve (bps)</text>\n");
    svg.push_str("</svg>\n");
    fs::write(output_dir.join("pareto.svg"), svg)?;
    Ok(())
}

fn write_recovery_svg(output_dir: &Path, points: &[RecoveryPoint]) -> EvalResult<()> {
    let max_y = points
        .iter()
        .flat_map(|point| [point.implementation_gap_bps, point.rolled_model_gap_bps])
        .max()
        .unwrap_or(1)
        .max(1);
    let mut svg = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1050\" height=\"650\" viewBox=\"0 0 1050 650\">\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n<text x=\"90\" y=\"30\" font-family=\"sans-serif\" font-size=\"18\">10% diagnostic quote: recovery parity</text>\n<line x1=\"90\" y1=\"560\" x2=\"750\" y2=\"560\" stroke=\"black\"/>\n<line x1=\"90\" y1=\"560\" x2=\"90\" y2=\"50\" stroke=\"black\"/>\n<text x=\"420\" y=\"625\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"16\">epochs since stress</text>\n<text x=\"25\" y=\"305\" transform=\"rotate(-90 25 305)\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"16\">gap from cleared-state quote (bps)</text>\n",
    );
    for step in 0..=2 {
        let x = 90.0 + 660.0 * (step as f64 / 2.0);
        svg.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"560\" x2=\"{x:.1}\" y2=\"566\" stroke=\"black\"/><text x=\"{x:.1}\" y=\"585\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"12\">{}</text>\n",
            step,
        ));
        let y = 560.0 - 510.0 * (step as f64 / 2.0);
        let value = max_y * step / 2;
        svg.push_str(&format!(
            "<line x1=\"84\" y1=\"{y:.1}\" x2=\"90\" y2=\"{y:.1}\" stroke=\"black\"/><text x=\"78\" y=\"{:.1}\" text-anchor=\"end\" font-family=\"sans-serif\" font-size=\"12\">{value}</text>\n",
            y + 4.0,
        ));
    }
    for (mechanism_index, mechanism) in ["prop_endpoint", "prop_pressure", "prop_full"]
        .iter()
        .enumerate()
    {
        let selected: Vec<_> = points
            .iter()
            .filter(|point| point.mechanism == *mechanism && point.quote_bps == 1_000)
            .collect();
        let color = ["#777777", "#1f77b4", "#d62728"][mechanism_index];
        for (field, dash) in [("implementation", ""), ("rolled_model", "7,5")] {
            let polyline = selected
                .iter()
                .map(|point| {
                    let x = 90.0 + 660.0 * (point.offset_quarters as f64 / 8.0);
                    let value = if field == "implementation" {
                        point.implementation_gap_bps
                    } else {
                        point.rolled_model_gap_bps
                    };
                    let y = 560.0 - 490.0 * (value as f64 / max_y as f64);
                    format!("{x:.1},{y:.1}")
                })
                .collect::<Vec<_>>()
                .join(" ");
            let opacity = if *mechanism == "prop_endpoint" {
                0.45
            } else {
                0.9
            };
            svg.push_str(&format!(
                "<polyline points=\"{polyline}\" fill=\"none\" stroke=\"{color}\" stroke-opacity=\"{opacity}\" stroke-width=\"2\" stroke-dasharray=\"{dash}\"><title>{mechanism} {field}</title></polyline>\n"
            ));
            let legend_y = 80 + mechanism_index * 55 + usize::from(field == "rolled_model") * 20;
            svg.push_str(&format!(
                "<line x1=\"790\" y1=\"{legend_y}\" x2=\"825\" y2=\"{legend_y}\" stroke=\"{color}\" stroke-width=\"2\" stroke-dasharray=\"{dash}\"/><text x=\"835\" y=\"{}\" font-family=\"sans-serif\" font-size=\"12\">{} {field}</text>\n",
                legend_y + 4,
                svg_escape(mechanism),
            ));
        }
    }
    let mismatch_count = points.iter().filter(|point| point.parity_gap != 0).count();
    svg.push_str(&format!(
        "<text x=\"790\" y=\"270\" font-family=\"sans-serif\" font-size=\"13\">nonzero parity points: {mismatch_count}</text>\n</svg>\n"
    ));
    fs::write(output_dir.join("recovery_parity.svg"), svg)?;
    Ok(())
}

fn output_directory() -> PathBuf {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("evaluation-output/prop-rfq"))
}

fn main() -> EvalResult<()> {
    let output_dir = output_directory();
    fs::create_dir_all(&output_dir)?;

    let mut events = Vec::new();
    let mut small_rows = Vec::new();
    let mut split_rows = Vec::new();
    let mut cadence_rows = Vec::new();
    let mut buy_rows = Vec::new();
    let mut ordering_rows = Vec::new();
    let mut recovery_rows = Vec::new();

    evaluate_small_after_stress(&mut small_rows, &mut events)?;
    let split_results = evaluate_splitting(&mut split_rows, &mut events)?;
    evaluate_cadence(&mut cadence_rows, &mut events)?;
    evaluate_buy_relief(&mut buy_rows, &mut events)?;
    evaluate_ordering(&mut ordering_rows, &mut events)?;
    let recovery_points = evaluate_recovery(&mut recovery_rows, &mut events)?;
    let parameter_sweep = evaluate_parameter_sweep()?;

    write_csv(
        &output_dir.join("events.csv"),
        "scenario,mechanism,variant,action,index,now,submitted,payout,executed,liquidity_before,liquidity_after,curr_sell,curr_buy,prev_net_sell,sell_count",
        &events,
    )?;
    write_csv(
        &output_dir.join("small_after_stress.csv"),
        "mechanism,stress_bps,small_bps,stress_payout,small_payout,small_execution_bps,small_discount_bps,reserve_remaining_bps,failed_exits",
        &small_rows,
    )?;
    write_csv(
        &output_dir.join("splitting.csv"),
        "mechanism,total_exit_bps,parts,timing,total_payout,one_shot_payout,split_gain_bps,reserve_remaining_bps,failed_parts",
        &split_rows,
    )?;
    write_csv(
        &output_dir.join("cadence.csv"),
        "mechanism,traffic,preliminary_trade_count,preliminary_trade_raw,preliminary_loss,quiet_victim_payout,victim_payout,victim_harm,cost_harm_ratio_bps,reserve_remaining_bps,failed_exits",
        &cadence_rows,
    )?;
    write_csv(
        &output_dir.join("buy_relief.csv"),
        "mechanism,mode,relief_bps_of_stress,funded_refill,nominal_buy,roundtrip_sell_payout,attacker_profit,victim_quote,reserve_remaining_bps,recorded_sell_pressure,recorded_buy_relief",
        &buy_rows,
    )?;
    write_csv(
        &output_dir.join("ordering.csv"),
        "mechanism,ordering,whale_payout,small_payout,total_payout,reserve_remaining_bps",
        &ordering_rows,
    )?;
    write_csv(
        &output_dir.join("recovery.csv"),
        "mechanism,quote_bps,offset_quarters,now,cleared_state_quote,implementation_quote,explicitly_rolled_model_quote,implementation_gap_bps,rolled_model_gap_bps,implementation_minus_model",
        &recovery_rows,
    )?;
    write_metadata(&output_dir)?;
    write_evaluation_contract(&output_dir)?;
    write_pareto_svg(&output_dir, &split_results)?;
    write_recovery_svg(&output_dir, &recovery_points)?;
    write_baseline_summary(&output_dir, &parameter_sweep)?;
    write_parameter_sweep(&output_dir, &parameter_sweep)?;
    write_parameter_sweep_svg(&output_dir, &parameter_sweep)?;

    println!("Prop RFQ evaluation written to {}", output_dir.display());
    println!(
        "Parameter sweep: {} total, {} pricing-eligible, {} security-eligible, {} fully eligible, {} frontier",
        parameter_sweep.rows.len(),
        parameter_sweep.ranked.len(),
        parameter_sweep.security_ranked.len(),
        parameter_sweep
            .rows
            .iter()
            .filter(|row| row.tradeoff_eligible)
            .count(),
        parameter_sweep.frontier.len(),
    );
    if let Some(best) = parameter_sweep.ranked.first() {
        println!(
            "Top mitigation candidate: {} (mitigation={} bps, small={} bps, split={} bps NAV, reserve={} bps, grief_pass={}, parity_gap={})",
            best.params.label(),
            best.cadence_mitigation
                .aggregate_split_mitigation_bps
                .map_or_else(|| "NA".to_string(), |value| value.to_string()),
            best.pricing.worst_small_discount_bps,
            best.pricing.max_split_gain_nav_bps,
            best.pricing.minimum_reserve_bps,
            best.grief_pass,
            best.recovery_parity_max_abs,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_off_has_zero_split_mitigation() -> EvalResult<()> {
        let params = PropParams::reference().pressure_only_for();
        let metrics = calculate_cadence_mitigation_metrics(params, params)?;

        assert_eq!(metrics.aggregate_split_mitigation_bps, Some(0));
        assert_eq!(metrics.worst_workload_split_mitigation_bps, Some(0));
        assert_eq!(metrics.improved_split_workloads, 0);
        assert_eq!(metrics.unchanged_split_workloads, 40);
        assert_eq!(metrics.worsened_split_workloads, 0);
        Ok(())
    }

    #[test]
    fn reference_cadence_mitigates_without_eliminating_split_gain() -> EvalResult<()> {
        let params = PropParams::reference();
        let metrics = calculate_cadence_mitigation_metrics(params, params.pressure_only_for())?;

        assert_eq!(metrics.aggregate_split_mitigation_bps, Some(967));
        assert_eq!(metrics.improved_split_workloads, 34);
        assert_eq!(metrics.unchanged_split_workloads, 6);
        assert_eq!(metrics.worsened_split_workloads, 0);
        Ok(())
    }
}
