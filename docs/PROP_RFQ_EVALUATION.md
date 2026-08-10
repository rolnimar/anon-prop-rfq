# Evaluation protocol

The evaluator is deterministic and uses no random workload generation.
Each scenario starts with 1,000,000,000 settlement base units, unit NAV, zero fees, and a protected order equal to 0.1% of initial liquidity.
The diagnostic configuration uses a 7% peg haircut, exponent 2.5, wall sensitivity 2.0, cadence threshold 20, cadence wave 1.0, and a 24-hour epoch.

The comparison contains fixed-NAV payout, seven static haircuts, an integrated cumulative curve, endpoint-only pricing, pressure without cadence, and the full mechanism.
The split matrix contains four exit sizes, five nontrivial split counts, and two timing patterns, for 40 equally weighted comparisons.
The parameter grid contains 10 haircuts, nine exponents, seven wall sensitivities, four cadence thresholds, and six cadence strengths, for 15,120 configurations.

The strict cadence diagnostic passes a dust trace only when the trace causes no later quote harm or the attacker's pricing loss is greater than the harm.
Transaction fees, priority fees, capital cost, and minimum-output failures are outside that diagnostic.
The paper reports the diagnostic as a limitation rather than treating it as a deployment recommendation.

The independent oracle implements pressure decay, dynamic-wall rounding, cadence rounding, and payout arithmetic directly in Python.
It uses 100-digit decimal arithmetic for the fractional endpoint power.
The SBF comparison tolerance is `max(3, ceil(raw_value / 50,000,000))` settlement base units.

The resource profile uses the fixed sample identifiers 0 through 99.
Each identifier deterministically derives the test keypairs used in that run.
This removes the accidental run-to-run variation caused by random account keys while retaining 100 distinct account layouts.

