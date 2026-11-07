# Tenant audit findings (C02 matrix + C03 lint)

C02 (tenant matrix) found no leaks; `crates/api/tests/known_leaks.txt` stays empty.
C03 flagged repo/API fns that touch tenant tables without a workspace predicate, relying on a
handler check alone. Fixed by taking `ws: Uuid` in the fn and putting it in the SQL (defense in depth).

| # | fn | query | fix | test | status |
|---|----|-------|-----|------|--------|
| 1 | `api::payouts::create_batch` + `db::payouts::owed` | program lock, owed sum, payout insert | `ws` param; `workspace_id` on program lock, owed, affiliate select | `leak_payout_create_batch_1` | status: closed |
| 2 | `api::payouts::{lock,approve_batch,cancel_batch,mark_paid}` | batch lock by id | `lock(c, ws, id)` joins `programs.workspace_id` | `leak_payout_batch_actions_2` | status: closed |
| 3 | `api::payouts::batch_csv` + `db::payouts::batch_rows` | batch existence, CSV rows | `ws` param, `pr.workspace_id = $2` | `leak_payout_batch_csv_3` | status: closed |
| 4 | `db::affiliates::count_codes` | codes per affiliate | `ws` param, join `affiliates.workspace_id` | `leak_db_code_and_delivery_fns_4` | status: closed |
| 5 | `db::programs::delete_offsite_codes` | delete codes of a program | `ws` param, `USING programs` workspace join | `leak_db_code_and_delivery_fns_4` | status: closed |
| 6 | `db::affiliates::delete_enrollment` (code delete) | delete codes | `USING programs` workspace join | existing enrollment tests | status: closed |
| 7 | `db::ops::requeue_delivery` | update delivery by id | `ws` param, join `webhook_endpoints.workspace_id` | `leak_db_code_and_delivery_fns_4` (compile guard) | status: closed |

No other flagged fn is reachable with a client-supplied id: the rest are system loops (worker,
dispatcher, rollup, tracker snapshot, secrets CLI) or self-scoped lookups, listed with reasons in
`docs/audit/sql-lint-allowlist.toml`.
