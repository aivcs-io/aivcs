.PHONY: bench bench-build

# Note: lci / local-ci is deprecated, not public, and has been removed.

# Build the release binary the bench harness drives. Separate target so CI can
# cache the build and only re-bench on rebuild.
bench-build:
	cargo build --release -p aivcs-cli

# Wall-clock bench of aivcs-cli hot paths via hyperfine.
# See tools/bench/aivcs-cli.sh for env overrides (AIVCS, BENCH_OUT, BENCH_RUNS).
bench: bench-build
	./tools/bench/aivcs-cli.sh
