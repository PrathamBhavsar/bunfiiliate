.PHONY: pentest ci fmt clippy test deny web openapi openapi-check e2e schemas-check js-test tracker-openapi-check

ci:
	cargo fmt --check
	cargo clippy --workspace -- -D warnings
	cargo test --workspace
	cargo deny check
	$(MAKE) web openapi-check tracker-openapi-check js-test

fmt:
	cargo fmt

clippy:
	cargo clippy --workspace -- -D warnings

test:
	cargo test --workspace

deny:
	cargo deny check


web:
	pnpm install --frozen-lockfile
	pnpm -C apps/web typecheck
	pnpm -C apps/web lint
	pnpm -C apps/web build

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

e2e:
	scripts/e2e.sh

schemas-check:
	npm --prefix scripts/schemas install --silent --no-audit --no-fund
	node scripts/validate-schemas.mjs

js-test:
	node scripts/size-check.mjs
	node --test scripts/ "sdks/js/test/*.test.ts"

pentest:
	scripts/pentest.sh

tracker-openapi-check:
	cargo run -q -p tracker -- openapi > /tmp/bf-tracker-openapi.json
	diff -u docs/openapi/tracker-v2.json /tmp/bf-tracker-openapi.json || { echo "tracker openapi drift: cargo run -q -p tracker -- openapi > docs/openapi/tracker-v2.json"; exit 1; }
