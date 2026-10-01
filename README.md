# Prop RFQ research artifact

This repository reproduces the evaluation of a pressure-aware request-for-quote mechanism for immediate exits from tokenized real-world-asset portfolios.
It contains the Rust implementation used by the host-side sweep, the rebuilt-SBF LiteSVM tests, an independent high-precision oracle, the fixed workloads, the resource profiler, and the committed reference results.

This artifact accompanies *Prop RFQ: Proprietary Request for Quote as Pressure-Aware Exit Pricing for Redeemable Real-World Asset Tokens* by Marian-Daniel Rolník, Ivan Homoliak, and Theodore Georgas.

## Origin and modifications

The program is derived from [OnRe's `onre-sol` implementation](https://github.com/onre-finance/onre-sol), copyright On Re Limited.
We acknowledge OnRe and the upstream contributors for the implementation on which this research artifact is based.
The paper's mechanism was developed with OnRe.

The research additions include the fixed adversarial workloads, pricing baselines and ablations, parameter sweep, independent Python oracle, LiteSVM parity and regression tests, resource profiling, plotting, and paper-claim verification.
The evaluated implementation includes the epoch-boundary recovery correction described in the paper; the negative-control patch restores the former behavior for comparison.
See [NOTICE.md](NOTICE.md) for attribution, licensing scope, and revision provenance.

The export retains the generic `rwa_exit` name and a separate program identifier originally used for anonymous review.
These identifiers are retained for reproducibility and do not identify the live OnRe deployment.
The export does not include the original development history, and its code and configuration should not be assumed to match the current upstream repository or production deployment.

## Licensing and citation

The software and research additions in this repository are distributed under the [MIT license](LICENSE.md).
OnRe's original copyright and permission notice is preserved; third-party dependencies remain subject to their own licenses.
The paper is licensed separately under **CC BY 4.0**.
The paper's Creative Commons license does not apply to software linked from the paper.

Citation metadata for the artifact and paper is provided in [CITATION.cff](CITATION.cff).
When reporting a reproduction, also identify the artifact commit and the generated run metadata.

## Evaluated revision

The evaluated implementation is tagged `R1` at commit `dc4999ea675b46e0ced77ab34ab100725df5d877`.
That commit is recorded in `results/reference/metadata.json` and `results/reference/resource_profile_metadata.json`.
Commit `e56cbf410f85850ca3ff93fcf2fa19e39c7b38db` packages the same program, tests, and evaluation harness with the committed reference outputs and updated verification tooling.
The `R1` tag and the packaged snapshot are separate root commits, not a parent-child sequence.
The historical reference metadata is retained unchanged.
Use the current checkout, which includes the restored license and attribution, to reproduce the experiments; regenerated metadata will record that checkout's commit and working-tree state.

## Security experiments

This artifact deliberately demonstrates order-splitting and cadence-griefing techniques in controlled evaluation workloads.
It also includes a negative control that restores a known recovery defect in an isolated worktree and checks that the oracle detects it.
These experiments document limitations of the mechanism and are intended for local research and reproducibility, not deployment against a live protocol.

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
