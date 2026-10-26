.PHONY: ci fmt clippy test deny

ci:
	cargo fmt --check
	cargo clippy --workspace -- -D warnings
	cargo test --workspace
	cargo deny check

fmt:
	cargo fmt

clippy:
	cargo clippy --workspace -- -D warnings

test:
	cargo test --workspace

deny:
	cargo deny check

