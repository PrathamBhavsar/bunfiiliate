# Release checklist

Branches: `prod` <- `staging` <- `feat/*`. Merge with `--no-ff` only. Never commit on `prod` or `staging`.

## Before any push

1. Integration branch off `staging`, merge the `feat/*` branches, regenerate `sdks/ts/openapi.json`,
   `docs/audit/tenant-inventory.md` and `crates/db/tests/checksums.txt` as needed.
2. `scripts/final-gate.sh` exits 0. It runs, in order and stopping at the first failure: `make ci`,
   `make sql-lint`, migrations check and upgrade test, the tenant matrix (zero skips), spool disk-full
   tests, alert rules check, `make backup-drill`, `make e2e` twice, `bench/run.sh --quick` (zero request
   errors), bench numbers check, `ocr review` on `prod..HEAD`, the brain validator, repo hygiene.
   Output: `.gate/report.md` plus one log per step.
3. Every `ocr` finding is fixed or recorded in `docs/audit/findings.md` with a reason.
4. Paste `.gate/report.md` into `brain/log.md`.
5. Free disk: `df -h` shows more than 15% free (a full disk makes the spool shed and e2e return 503).

## Push

- `scripts/push-guard.sh` must pass: remote is `github.com/pratham-projects/*`, clean tree, passing
  `.gate/report.md`, no force flags. It refuses anything else.
- Push `staging`, then merge `staging -> prod --no-ff` and push `prod`. Tag `vX.Y.Z` after the push.
- No push until the user says so.

## After

- Smoke `/healthz`, `/readyz`, `/metrics` on the new build; run `make backup-drill` against a copy of prod data.
- Delete merged `feat/*` branches and their worktrees.
