.PHONY: ci fmt clippy test

ci:
	cargo fmt --check
	cargo clippy --workspace -- -D warnings
	cargo test --workspace

fmt:
	cargo fmt

clippy:
	cargo clippy --workspace -- -D warnings

test:
	cargo test --workspace
