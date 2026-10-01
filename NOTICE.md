# Attribution and provenance

## Upstream implementation

This repository contains software derived from [OnRe's `onre-sol` repository](https://github.com/onre-finance/onre-sol).
The upstream copyright notice is:

> Copyright (c) 2025 On Re Limited

The full upstream MIT permission notice and warranty disclaimer are preserved in [LICENSE.md](LICENSE.md).
This acknowledgement supplements that license notice and does not replace it.
Existing third-party notices must also be retained where applicable.

The paper cites upstream revision [`fc11256d9f411494f849931dcd7ae801a50ca29c`](https://github.com/onre-finance/onre-sol/commit/fc11256d9f411494f849931dcd7ae801a50ca29c) as a public implementation reference.
That citation is not a claim that this artifact is an unmodified export of that revision.

## Research modifications and authorship

The artifact adds evaluation workloads, baselines, parameter sweeps, an independent reference oracle, parity and regression tests, resource measurements, result files, and reproduction tooling.
It includes the recovery correction evaluated in the paper and a separate patch used to test the former defect.
The export also replaces identifying program names and the program identifier used during review.

The associated paper is authored by Marian-Daniel Rolník, Ivan Homoliak, and Theodore Georgas and describes work developed with OnRe.
This statement credits the paper's authors without assigning sole ownership of the upstream implementation or each research addition to any individual author.

## Licensing scope

The MIT license in [LICENSE.md](LICENSE.md) applies to this repository's software and research additions unless a file carries a separate notice.
Third-party dependencies retain their respective licenses.
The associated paper is licensed under CC BY 4.0; that paper license does not relicense this repository or the upstream OnRe code.

## Recorded revisions

| Identifier | Meaning |
| --- | --- |
| `dc4999ea675b46e0ced77ab34ab100725df5d877` (`R1` tag) | Evaluated source implementation recorded by the committed reference experiments |
| `e56cbf410f85850ca3ff93fcf2fa19e39c7b38db` | Packaged snapshot with the same program, tests, and evaluation harness, plus reference outputs and updated verification tooling |
| `fc11256d9f411494f849931dcd7ae801a50ca29c` | Public upstream implementation revision cited in the paper |

The `R1` tag and the packaged snapshot are separate root commits in this repository, so these identifiers are not interchangeable.
The public upstream revision belongs to the separate OnRe repository.
Documentation and licensing updates do not replace the historical metadata in `results/reference`.
New runs record their own checkout and working-tree state.
When redistributing a historical snapshot separately, include the restored `LICENSE.md` and this notice from the current checkout; the original review snapshots omitted those files.
