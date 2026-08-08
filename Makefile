OUTPUT_DIR ?= results/generated
REFERENCE_DIR ?= results/reference
RESOURCE_SAMPLES ?= 100
PROGRAM_MANIFEST := programs/rwa_exit/Cargo.toml
PROGRAM_TEST := prop_rfq

.DEFAULT_GOAL := help
.NOTPARALLEL:

.PHONY: help reproduce oracle-vectors build integration oracle-parity scenarios resources figures verify verify-reference negative-control anonymity

help:
	@echo "make reproduce         Rebuild and regenerate every paper experiment"
	@echo "make verify-reference  Check the committed result snapshot"
	@echo "make negative-control  Reproduce the former recovery defect"
	@echo "make anonymity         Scan tracked artifact content for identifying names"

reproduce: oracle-vectors build integration oracle-parity scenarios resources figures verify anonymity

oracle-vectors:
	python3 scripts/prop_rfq_reference_oracle.py --check

build:
	bash scripts/prepare-anchor-sbpf-toolchain.sh
	anchor build

integration:
	cargo test --manifest-path $(PROGRAM_MANIFEST) --test $(PROGRAM_TEST)

oracle-parity:
	python3 scripts/run_oracle_verification.py --output $(OUTPUT_DIR)/oracle_verification.json

scenarios:
	cargo run --release --manifest-path $(PROGRAM_MANIFEST) --example prop_rfq_evaluation -- $(OUTPUT_DIR)

resources:
	python3 scripts/profile_prop_rfq_resources.py --samples $(RESOURCE_SAMPLES) --output $(OUTPUT_DIR)

figures:
	uv run scripts/plot_prop_rfq_evaluation.py \
		--input $(OUTPUT_DIR)/parameter_sweep.csv \
		--small-after-stress $(OUTPUT_DIR)/small_after_stress.csv \
		--output-pdf $(OUTPUT_DIR)/parameter_sweep_readable.pdf \
		--output-svg $(OUTPUT_DIR)/parameter_sweep_readable.svg

verify:
	python3 scripts/verify_paper_claims.py --results $(OUTPUT_DIR) --expected-resource-samples $(RESOURCE_SAMPLES)

verify-reference:
	python3 scripts/verify_paper_claims.py --results $(REFERENCE_DIR) --expected-resource-samples 100

negative-control:
	bash scripts/verify_prop_rfq_recovery_negative_control.sh

anonymity:
	python3 scripts/check_anonymity.py

