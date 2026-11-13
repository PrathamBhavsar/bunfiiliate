// bunfiiliate: all site copy and config. Every claim here is traceable to the repo (README, docs/benchmarks.md, docs/ledger.md).
export default {
  name: "bunfiiliate",
  wordmark: "bunf<b>ii</b>liate",
  repo: "PrathamBhavsar/bunfiiliate",
  scene: "rain",
  shot: { name: "analytics", only: "dark", alt: "The bunfiiliate admin Analytics page: clicks, conversions and commission over 30 days, a country breakdown, and the acid-green accent on a dark theme." },
  license: "AGPL-3.0",
  stack: "Rust · Postgres 16",
  status: "pre-release",
  headline: "Affiliate tracking that <em>adds up.</em>",
  sub: "Open-source tracking and commission engine in Rust. Clicks never touch the database on the hot path. Money lives in an <strong>append-only double-entry ledger</strong> in integer cents.",
  heroCmdPre: "git clone https://github.com/PrathamBhavsar/bunfiiliate && cd bunfiiliate",
  heroCmd: "curl -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/api-installer.sh | sh",

  liveTitle: "Project facts.",
  liveNote: "GitHub numbers show up here when GitHub answers. The rest is read from files in the repo. Benchmarks come from one laptop, so read `docs/benchmarks.md` before quoting them.",
  liveLinks: [["Benchmarks", "docs/benchmarks.md"], ["Ledger spec", "docs/ledger.md"]],
  facts: [
    ["Workspace", "6 crates · 2 binaries"],
    ["Tracker p50, fsync off", "0.4 ms · 27.6k req/s"],
    ["Tracker p50, fsync always", "13 ms · 1.5k req/s"],
    ["tracker.js", "under 2 KB minified"],
    ["Ledger", "append-only, enforced by Postgres"],
    ["License", "AGPL-3.0, clean-room"],
  ],

  featuresTitle: "What is in it.",
  featuresLede: "There are no tiers and no take-rate. It is one Rust workspace that builds two binaries and talks to one Postgres.",
  features: [
    { title: "Tracker", blurb: "`tracker` handles the hot path and answers clicks without a database call.", items: [
      ["flash", "Zero DB calls per request", "Codes and keys are checked against an in-memory snapshot, reloaded on Postgres `LISTEN bf_config`."],
      ["disk", "Crash-safe WAL spool", "Segmented write-ahead log with CRC32 framing, group-commit fsync (`always` or `off`), atomic checkpoints, replay on restart."],
      ["sync", "Batched writer", "Acked batches flush to Postgres with `INSERT … UNNEST … ON CONFLICT DO NOTHING`. At-least-once plus unique keys is effectively-once."],
      ["alert", "Load shedding", "Bounded buffers return HTTP 503 under pressure instead of growing memory without limit."],
      ["code", "Client script under 2 KB", "`tracker.min.js` sets first-party `bf_vid` and `bf_ref` cookies and sends click beacons."],
      ["webhook", "Webhook ingestion", "Stripe, Paddle and generic HMAC events, verified in constant time with a 5-minute tolerance and idempotency keys."],
    ]},
    { title: "Money", ledger: true, blurb: "Balances are derived from ledger entries and are not stored.", items: [
      ["wallet", "Double-entry ledger", "Accounts: Expense, AffPending, AffPayable, Cash. Every movement posts a balanced transaction."],
      ["lock", "Append-only by trigger", "Postgres rejects `UPDATE` and `DELETE` on `ledger_entries`. A deferrable constraint enforces `sum(amount) = 0` per transaction."],
      ["coins", "Integer cents, basis points", "It uses no floats. Commission is `i128` basis-point math with round-half-up, as written in `docs/ledger.md`."],
      ["layers", "Commission plans", "Flat or percent, for one month, X months or lifetime. Affiliate override beats conversion type beats program default."],
    ]},
    { title: "Programs", blurb: "Everything is scoped by workspace, then program, affiliate and code.", items: [
      ["users", "Multi-tenant", "Roles `super_admin`, `admin`, `affiliate`. Workspace scope on every query. Affiliate portal included."],
      ["branch", "Attribution models", "First-touch and last-touch, per program."],
      ["shield2", "Traffic hygiene", "IP velocity limits, bot user-agent filtering, click dedupe, salted IP hashing with rotation."],
      ["link", "Referral codes and landings", "`/r/:code` records the click and redirects with `?bf_click=`. Multiple sites per program."],
      ["globe", "Tax forms and currencies", "Affiliates record a W-8 or W-9 (only the last 4 digits of a tax id are stored). One currency per program; the worker fetches daily exchange rates."],
    ]},
    { title: "Payouts and webhooks", blurb: "From payable balance to a bank file, with an approval step.", items: [
      ["money", "Payout batches", "Built from payable balances above each affiliate's minimum. Admin approval gate. Bank CSV export."],
      ["card", "Payout rails", "Mark paid by hand from the CSV, or send through PayPal Payouts or a Wise adapter. Retries are idempotent per payout line. The Wise adapter has not been run against Wise's sandbox yet."],
      ["send", "Transactional outbox", "Signed outbound webhooks with exponential backoff and a dead state. Emails via lettre and minijinja templates."],
      ["analytics", "Rollups and reports", "Daily rollups by country, OS, browser, device and referrer. CSV reports."],
    ]},
    { title: "Security", blurb: "How sessions, passwords and secrets are handled.", items: [
      ["shield", "Cookie sessions", "HttpOnly `SameSite=Strict` access JWT, rotating refresh token. Reuse of an old token revokes the whole family."],
      ["finger", "Passwords and 2FA", "Argon2 hashing, optional TOTP with recovery codes, rate-limited non-enumerating reset."],
      ["lock", "Secrets at rest", "Webhook and provider secrets encrypted with a rotating master key (`BF_MASTER_KEYS`)."],
    ]},
    { title: "Operate it", blurb: "It runs on one VPS.", items: [
      ["server", "Two binaries, one image", "`api` and `tracker`. Docker Compose or systemd behind Caddy. Backups and monitoring configs in `deploy/`."],
      ["activity", "Prometheus metrics", "API on 9100, tracker on 9101. OpenAPI at `/api/openapi.json`, typed TypeScript SDK."],
      ["chart", "Benchmarked, with caveats", "`bench/` scripts and `docs/benchmarks.md` publish both fsync modes with the hardware they ran on."],
    ]},
  ],

  howTitle: "What happens to one click.",
  howLede: "The tracker acknowledges a click after the spool fsync. Attribution, pricing and the ledger entry happen later, in one transaction.",
  steps: [
    ["Visitor clicks `/r/code`", "The tracker checks the code against its in-memory snapshot and answers with a 302. No database round trip."],
    ["Event enters the WAL spool", "Appended to a segmented log. The handler waits for the group-commit fsync, then acks."],
    ["Batch writer flushes to Postgres", "Bounded channel, `UNNEST` inserts, idempotent on conflict. The checkpoint advances and old segments go."],
    ["Worker attributes and prices", "Fraud rules, first or last touch, commission plan. Claimed with `FOR UPDATE SKIP LOCKED`."],
    ["Ledger posts, outbox fires", "A balanced transaction, the conversion row and the outbox message commit together or not at all."],
  ],

  installTitle: "Install.",
  installLede: "Download a release binary, run the image, or build from source. Checksums are published with every release.",
  installLedePending: "Run it with Docker Compose or build from source. Release binaries and checksums arrive with the first tagged release.",
  install: [
    { id: "docker", label: "Docker", cmds: [
      "git clone https://github.com/PrathamBhavsar/bunfiiliate && cd bunfiiliate",
      "cp .env.example .env   # set JWT_SECRET, BF_IP_SALT, BF_MASTER_KEYS, SITE_ADDRESS",
      "docker compose --env-file .env -f deploy/docker-compose.yml up -d",
    ], note: "Postgres, api, tracker and Caddy with automatic TLS. Caddy serves the UI and `/api` on one origin." },
    { id: "source", label: "Source", cmds: [
      "git clone https://github.com/PrathamBhavsar/bunfiiliate && cd bunfiiliate",
      "cargo build --release -p api -p tracker",
      "cargo run -p api -- serve --with-worker",
    ], note: "Rust 1.80+ and PostgreSQL 16. See the quickstart for the first admin and a demo seed." },
    { id: "binary", label: "Binary", cmds: [
      "curl -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/api-installer.sh | sh",
      "curl -LsSf https://github.com/PrathamBhavsar/bunfiiliate/releases/latest/download/tracker-installer.sh | sh",
      "api bootstrap --email you@example.com --workspace \"Acme\"",
      "api serve --with-worker & tracker",
    ], note: "Needs PostgreSQL 16 and `DATABASE_URL`. macOS and Linux, x86_64 and arm64.", pending: "No release is tagged yet: these URLs work from the first release. Use Docker or source today." },
  ],

  faq: [
    ["Is it production ready?", "Not yet. It is pre-release and under active development. The ledger and tracker have property tests and benchmarks, but there is no tagged release yet."],
    ["What does sub-millisecond mean here?", "With `BF_FSYNC=off` the tracker measured 0.4 ms p50 on one machine. With `BF_FSYNC=always`, which survives a host crash, p50 was about 13 ms. Both numbers, and the hardware, are in `docs/benchmarks.md`."],
    ["How do affiliates get paid?", "bunfiiliate builds the payout batch and a bank CSV. Pay by hand and mark the batch paid, or use a rail: PayPal Payouts, or the Wise adapter (not yet verified against Wise's sandbox). Rail credentials are stored per workspace, encrypted."],
    ["Which license?", "AGPL-3.0. It is a clean-room implementation: no code, schema text or copy from any closed-source project."],
    ["How is the ledger protected?", "Postgres enforces it: updates and deletes are rejected by trigger, and each transaction must sum to zero or the commit fails. Read `docs/ledger.md`."],
  ],

  footBlurb: "Open-source affiliate tracking and commission engine in Rust.",
  footNote: "Pre-release. The numbers come from benchmarks run on one laptop.",
};
