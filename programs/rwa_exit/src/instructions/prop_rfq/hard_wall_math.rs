use anchor_lang::prelude::*;

use super::config::{CURVE_EXPONENT_SCALE, CURVE_EXPONENT_STEP};

pub const HARD_WALL_SCALE: u128 = 1_000_000_000_000;

// Prop RFQ sell dampening needs `u^e`, where:
//   u = raw_sell_value / effective_liquidity
//   e = curve_exponent_scaled / CURVE_EXPONENT_SCALE
//
// Both `u` and the return value are scaled by HARD_WALL_SCALE. Integer exponents
// use repeated fixed-point multiplication with saturation at extreme values.
// Fractional exponents use:
//   u^e = 2^(e * log2(u))
//
// The approximation is intentionally table-free:
// - `log2_integer_q` normalizes the input to mantissa `m` in [1, 2), then uses
//   ln(m) = 2 * (z + z^3/3 + z^5/5 + ...), z = (m - 1) / (m + 1), converted by
//   log2_e. Seven odd terms total are used: z through z^13/13.
// - `exp2_hard_wall_scaled_q` splits the exponent into integer and fractional
//   parts, computes exp(frac * ln(2)) with ten Taylor terms, then applies the
//   integer power-of-two shift.
//
// Q40 is used inside the approximation to keep enough precision while staying
// cheap in compute units compared with generic nth-root or table interpolation.
const POW_APPROX_Q_SHIFT: u32 = 40;
const POW_APPROX_Q: u128 = 1_u128 << POW_APPROX_Q_SHIFT;
const POW_APPROX_LN2_Q: u128 = 762_123_384_786;
const POW_APPROX_LOG2_E_Q: u128 = 1_586_259_972_792;
const LOG2_HARD_WALL_SCALE_Q: i128 = 43_829_982_801_540;

pub(crate) fn bps_to_hard_wall_scale(bps: u16) -> Result<u128> {
    Ok(HARD_WALL_SCALE
        .checked_mul(bps as u128)
        .ok_or(crate::ProtocolError::MathOverflow)?
        .checked_div(crate::constants::MAX_BASIS_POINTS as u128)
        .ok_or(crate::ProtocolError::DivByZero)?)
}

pub(crate) fn utilization_power_scaled(u: u128, exponent_scaled: u32) -> Result<u128> {
    validate_curve_exponent_scaled(exponent_scaled)?;
    if exponent_scaled == 0 {
        return Ok(HARD_WALL_SCALE);
    }
    if u == 0 {
        return Ok(0);
    }
    if u == HARD_WALL_SCALE {
        return Ok(HARD_WALL_SCALE);
    }
    if exponent_scaled.is_multiple_of(CURVE_EXPONENT_SCALE) {
        return Ok(integer_utilization_power_scaled(
            u,
            exponent_scaled / CURVE_EXPONENT_SCALE,
        ));
    }

    let log2_u_q = log2_hard_wall_scaled_q(u);
    let exponentiated_log_q = log2_u_q
        .checked_mul(exponent_scaled as i128)
        .ok_or(crate::ProtocolError::MathOverflow)?
        .checked_div(CURVE_EXPONENT_SCALE as i128)
        .ok_or(crate::ProtocolError::DivByZero)?;
    Ok(exp2_hard_wall_scaled_q(exponentiated_log_q))
}

pub(crate) fn validate_curve_exponent_scaled(exponent_scaled: u32) -> Result<()> {
    require!(
        exponent_scaled <= CURVE_EXPONENT_SCALE.saturating_mul(10),
        crate::ProtocolError::InvalidAmount
    );
    require!(
        exponent_scaled.is_multiple_of(CURVE_EXPONENT_STEP),
        crate::ProtocolError::InvalidAmount
    );
    Ok(())
}

fn integer_utilization_power_scaled(u: u128, exponent: u32) -> u128 {
    let mut value = HARD_WALL_SCALE;
    for _ in 0..exponent {
        value = mul_scaled_saturating(value, u);
    }
    value
}

fn log2_hard_wall_scaled_q(value: u128) -> i128 {
    log2_integer_q(value) - LOG2_HARD_WALL_SCALE_Q
}

fn log2_integer_q(value: u128) -> i128 {
    let msb = (u128::BITS - 1 - value.leading_zeros()) as i128;
    let shift = msb - POW_APPROX_Q_SHIFT as i128;
    let mantissa_q = if shift >= 0 {
        value >> shift as u32
    } else {
        value << (-shift as u32)
    };
    let z = mantissa_q
        .saturating_sub(POW_APPROX_Q)
        .saturating_mul(POW_APPROX_Q)
        / mantissa_q.saturating_add(POW_APPROX_Q);
    let z2 = (z.saturating_mul(z)) >> POW_APPROX_Q_SHIFT;

    let mut term = z;
    let mut sum = term;
    for divisor in [3_u128, 5, 7, 9, 11, 13] {
        term = (term.saturating_mul(z2)) >> POW_APPROX_Q_SHIFT;
        sum = sum.saturating_add(term / divisor);
    }

    let ln_mantissa_q = sum.saturating_mul(2);
    let fractional_q = (ln_mantissa_q.saturating_mul(POW_APPROX_LOG2_E_Q)) >> POW_APPROX_Q_SHIFT;
    msb.saturating_mul(POW_APPROX_Q as i128)
        .saturating_add(fractional_q as i128)
}

fn exp2_hard_wall_scaled_q(log2_value_q: i128) -> u128 {
    let q = POW_APPROX_Q as i128;
    let mut integer_part = log2_value_q / q;
    let mut fractional_part = log2_value_q % q;
    if fractional_part < 0 {
        fractional_part += q;
        integer_part -= 1;
    }

    let x_q = ((fractional_part as u128).saturating_mul(POW_APPROX_LN2_Q)) >> POW_APPROX_Q_SHIFT;
    let mut term = POW_APPROX_Q;
    let mut exp_fraction_q = POW_APPROX_Q;
    for divisor in [1_u128, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
        term = (term.saturating_mul(x_q)) >> POW_APPROX_Q_SHIFT;
        term /= divisor;
        exp_fraction_q = exp_fraction_q.saturating_add(term);
    }

    let scaled = (exp_fraction_q.saturating_mul(HARD_WALL_SCALE)) >> POW_APPROX_Q_SHIFT;
    if integer_part >= 0 {
        let shift = integer_part as u32;
        if shift >= u128::BITS {
            return u128::MAX;
        }
        if scaled > (u128::MAX >> shift) {
            return u128::MAX;
        }
        return scaled << shift;
    }

    let shift = (-integer_part) as u32;
    if shift >= u128::BITS {
        0
    } else {
        scaled >> shift
    }
}

fn mul_scaled_saturating(lhs: u128, rhs: u128) -> u128 {
    lhs.saturating_mul(rhs)
        .checked_div(HARD_WALL_SCALE)
        .unwrap_or(u128::MAX)
}
