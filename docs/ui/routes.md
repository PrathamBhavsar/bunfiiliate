# Web routes

Static export (`apps/web`, ADR-0006). Detail pages take the id in the query string.

| Page | URL | Status |
|---|---|---|
| Dashboard | `/dashboard` | existing |
| Programs / Affiliates / Conversions / Customers / Ledger / Payouts | `/programs` ... (list pages; row selection uses `?id=` via resource-view) | existing |
| Referral codes | `/referral-codes` | existing |
| Referral code stats (admin) | `/referral-codes/detail?code=<code>` | C31 |
| My links | `/me/links` | existing |
| My link stats (affiliate) | `/me/links/detail?code=<code>` | C31 |
| Webhooks, Notifications, Audit log, Contact requests, Email templates, Reports, Analytics | `/<name>` | existing |
| Settings, Billing | `/settings`, `/settings/billing` | existing |
| Me: home, earnings, payouts, profile | `/me`, `/me/earnings`, `/me/payouts`, `/me/profile` | existing |
| Auth | `/login`, `/register?workspace=`, `/signup`, `/forgot-password`, `/reset-password` | existing |

Caddy: `try_files {path} {path}.html {path}/ /index.html`; `trailingSlash` is unset.
