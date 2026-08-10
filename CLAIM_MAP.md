# Paper claim map

Every quantitative evaluation claim has an executable source and a committed output.
`make reproduce` regenerates the outputs, and `scripts/verify_paper_claims.py` checks the values below.

| Paper result | Command | Primary output |
| --- | --- | --- |
| Fixed-NAV, static, integrated, endpoint, pressure, and full baselines | `make scenarios` | `baseline_summary.csv` |
| Four exit sizes, five nontrivial split counts, and two timing patterns | `make scenarios` | `evaluation_contract.json`, `splitting.csv` |
| 15,120-row Cartesian parameter sweep | `make scenarios` | `parameter_sweep.csv` |
| Dominance, pricing, security, and frontier counts | `make scenarios` | `parameter_ranked.csv`, `parameter_security_ranked.csv`, `parameter_frontier.csv` |
| Dust cadence manipulation and honest bursts | `make scenarios` | `cadence.csv` |
| Funded buy relief and buy-then-sell loss | `make scenarios` | `buy_relief.csv` |
| Stress-first and protected-order-first permutations | `make scenarios` | `ordering.csv` |
| Epoch-boundary and two-epoch recovery parity | `make scenarios` | `recovery.csv` |
| 34 independent pricing vectors and maximum absolute difference | `make oracle-parity` | `oracle_verification.json`, `prop_rfq_oracle_vectors.csv` |
| Five decimal-conversion vectors | `make oracle-parity` | `oracle_verification.json`, `prop_rfq_decimal_vectors.csv` |
| Former recovery defect | `make negative-control` | Expected oracle failure reported by the command |
| Quote and execution compute units | `make resources` | `resource_profile.csv` |
| Pair-state size and rent | `make resources` | `resource_accounts.csv` |
| Transaction sizes and the 1,232-byte packet check | `make resources` | `resource_profile.csv` |
| Toolchain, platform, sample count, and deterministic seed scheme | `make resources` | `resource_profile_metadata.json` |
| Event-level traces for independent analysis | `make scenarios` | `events.csv` and workload-specific CSV files |
| Three-panel parameter figure | `make figures` | `parameter_sweep_readable.pdf`, `parameter_sweep_readable.svg` |

The host sweep calls the same Rust pricing and state-transition functions as the program.
The independent Python oracle does not import Rust code or generated program interfaces.
The LiteSVM parity tests execute the rebuilt SBF binary, so they check the host model against a separate implementation path.

