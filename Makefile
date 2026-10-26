.PHONY: ci fmt clippy test deny openapi openapi-check

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


openapi:
	cargo run -q -p api -- openapi > sdks/ts/openapi.json
	pnpm -C sdks/ts generate

openapi-check:
	@tmp=$$(mktemp -d) && trap 'rm -rf $$tmp' EXIT && \
	cargo run -q -p api -- openapi > $$tmp/openapi.json && \
	pnpm -C sdks/ts exec openapi-typescript $$tmp/openapi.json -o $$tmp/schema.d.ts > /dev/null && \
	diff -u sdks/ts/openapi.json $$tmp/openapi.json && \
	diff -u sdks/ts/src/schema.d.ts $$tmp/schema.d.ts || \
	{ echo "openapi drift: run 'make openapi' and commit"; exit 1; }
