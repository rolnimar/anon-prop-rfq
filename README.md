# Anonymous experiment artifact

This repository reproduces the evaluation of a pressure-aware request-for-quote mechanism for immediate exits from tokenized real-world-asset portfolios.
It contains the Rust implementation used by the host-side sweep, the rebuilt-SBF LiteSVM tests, an independent high-precision oracle, the fixed workloads, the resource profiler, and the committed reference results.

The source uses generic names and a fresh program identifier for double-anonymous review.
The implementation logic is otherwise the revision evaluated in the paper.
The original project history and identifying metadata are not included.

## Requirements

The reference run used:

- Anchor CLI 1.1.2;
- Solana CLI 4.0.2 and platform tools 1.53;
- Rust 1.97.1 for host tests and Rust 1.89.0 for SBF;
- Python 3.14.3; and
- uv 0.11.6.

Python 3.11 or newer is sufficient for the scripts.
The first SBF build downloads dependencies from the locked Cargo registry sources.

## Reproduce the paper results

Run:

```bash
make reproduce
```

The target performs these steps in order:

1. checks that the committed oracle vectors match the independent Python model;
2. rebuilds the SBF program;
3. runs all 49 Prop RFQ LiteSVM tests;
4. runs the 34 pricing-vector and five decimal-vector parity checks;
5. evaluates the fixed workloads and all 15,120 parameter configurations;
6. profiles 100 deterministic SBF executions;
7. regenerates the readable sweep figure; and
8. checks the generated files against the numerical claims in the paper.

Generated files are written to `results/generated`.
The frozen output used for the paper is under `results/reference`.

To check the committed snapshot without rebuilding SBF, run:

```bash
make verify-reference
```

## Negative control

The recovery regression has a separate negative control:

```bash
make negative-control
```

It creates an isolated worktree, restores the former epoch-boundary defect, rebuilds SBF, and succeeds only when the independent oracle detects the expected second-epoch mismatch.
The repository must be clean before this command is run.

## Artifact layout

| Path | Purpose |
| --- | --- |
| `programs/rwa_exit/src` | Anchor implementation used by the SBF tests and host evaluator |
| `programs/rwa_exit/examples/prop_rfq_evaluation.rs` | Fixed workloads, baselines, parameter sweep, gates, and CSV writers |
| `programs/rwa_exit/tests/prop_rfq.rs` | LiteSVM integration, parity, regression, and resource tests |
| `programs/rwa_exit/tests/fixtures` | 34 pricing vectors and five decimal-conversion vectors |
| `scripts/prop_rfq_reference_oracle.py` | Independent 100-digit decimal reference model |
| `scripts/profile_prop_rfq_resources.py` | Deterministic 100-sample compute and transaction profiler |
| `scripts/verify_paper_claims.py` | Machine-readable claim-to-output checks |
| `CLAIM_MAP.md` | Paper claim to command and file mapping |
| `results/reference` | Frozen reference output |

The license notice is intentionally withheld from the anonymous review artifact because it identifies the authors.
It must be restored before any non-review publication of the source.

