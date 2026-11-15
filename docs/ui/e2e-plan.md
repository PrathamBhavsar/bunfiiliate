# E2E coverage plan

Every page under `apps/web/src/app` and the e2e test that drives it. `scripts/e2e-plan.test.mjs` (run by `make ci` via `js-test`) fails when a page is missing here or a named test is gone.

Suites live in `e2e/tests/` and seed their own data through `e2e/fixtures/seed.ts` (unique suffix per run), so each file passes alone: `E2E_GREP=<title regex> make e2e` narrows the run, `E2E_SHARD=1/2` slices it. States and axe cover every list page in `states-a11y.e2e.ts`.

| Page | Test file | Test |
|---|---|---|
| `/` | `states-a11y.e2e.ts` | axe_no_serious_violations_login |
| `/affiliates` | `admin-core.e2e.ts` | the admin approves and enrolls them |
| `/analytics` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/audit-log` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/contact-requests` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/conversions` | `money.e2e.ts` | a signed conversion is ingested |
| `/customers` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/dashboard` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/email-templates` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/forgot-password` | `admin-core.e2e.ts` | reset page rejects a bad token |
| `/ledger` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/login` | `states-a11y.e2e.ts` | axe_no_serious_violations_login |
| `/me` | `portal.e2e.ts` | every portal page renders its heading |
| `/me/earnings` | `money.e2e.ts` | the affiliate sees the paid commission |
| `/me/links` | `portal.e2e.ts` | the affiliate creates a referral code |
| `/me/links/detail` | `code-stats.e2e.ts` | the affiliate sees stats for their own code |
| `/me/payouts` | `portal.e2e.ts` | the payout request form checks the amount |
| `/me/profile` | `portal.e2e.ts` | every portal page renders its heading |
| `/notifications` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/payouts` | `money.e2e.ts` | admin sees a pending request and approves it |
| `/programs` | `admin-core.e2e.ts` | admin creates a program with a site |
| `/referral-codes` | `tenant-ui.e2e.ts` | ws_a_admin_lists_never_show_ws_b_rows |
| `/referral-codes/detail` | `code-stats.e2e.ts` | code_stats_page_renders_kpis |
| `/register` | `admin-core.e2e.ts` | a visitor registers as an affiliate |
| `/reports` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/reset-password` | `admin-core.e2e.ts` | reset page rejects a bad token |
| `/settings` | `admin-core.e2e.ts` | settings stores a channel URL write-only |
| `/settings/billing` | `admin-core.e2e.ts` | billing page shows the plan and usage |
| `/signup` | `admin-core.e2e.ts` | a visitor registers as an affiliate |
| `/webhooks` | `admin-core.e2e.ts` | every admin page renders its heading |
| `/webhooks/test` | `money.e2e.ts` | webhook tester sends a signed test event |

Suites: `admin-core` (shell, setup, page smoke), `money` (ingest, batches, rails, tester, payout requests), `portal` (affiliate pages), `code-stats`, `states-a11y`, `tenant-ui` (workspace A vs B).
