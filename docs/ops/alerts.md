# Alerts

Optional stack in `deploy/monitoring/` (Prometheus + Alertmanager, images `prom/prometheus` and `prom/alertmanager` only):

```bash
openssl rand -hex 24 > deploy/monitoring/alert-token     # same value as BF_ALERT_TOKEN
docker compose -f deploy/compose.yml -f deploy/monitoring/compose.monitoring.yml --profile monitoring up -d
```

The tracker and api expose `/metrics` on `127.0.0.1:9100` / `:9101` (`BF_METRICS_ADDR`, optional `BF_METRICS_TOKEN` bearer). Prometheus in a container needs them reachable on a private network (and the token set); never expose them publicly.

## Delivery: in-app, super admins only

Alertmanager posts to `POST /api/v1/alerts/webhook` on the api with `Authorization: Bearer $BF_ALERT_TOKEN`.

- `BF_ALERT_TOKEN` unset (or empty) = the route is disabled and answers 404.
- The token is compared in constant time. A wrong or missing token is 401. Body limit 64 KiB (413), at most 50 alerts per call.
- Each alert becomes an in-app notification for **every super admin** (kind `alert`). A `resolved` alert adds `alert_resolved` and marks the earlier unread firing notifications with the same `[alertname]` as read. Workspace admins and affiliates get nothing.
- SMTP/Slack receivers are commented in `alertmanager.yml`; nothing needs an email service.

## Rules

`deploy/monitoring/rules/bunfiiliate.rules.yml`. Only metrics the binaries export are used: the test `rules_reference_only_exported_metrics` (`cargo test -p api --test alerts`) fails on an unknown `bf_*` name. `scripts/check-alerts.sh` runs `promtool check rules`, `promtool test rules` and `amtool check-config` when installed (else docker; skipped with a message if neither).

Not alerted yet because the metric does not exist: API 5xx rate, batch writer errors, Postgres down, migration pending. `/readyz` on the api and tracker covers PG and migrations for an external probe. Adding the metrics comes first, then the rule. `bf_clicks_default_rows` arrives with the partitioning task (C20).

## Runbook

### TrackerDown
Tracker process or its metrics port is not answering. `systemctl status bunfiiliate-tracker`, logs, `/healthz`. Clicks are lost while it is down.

### ApiDown
Same for the api / worker.

### SpoolShedding
Spool disk is full; see `docs/ops/spool-disk.md`.

### SpoolDiskLow
Under 1 GiB free on the spool filesystem. Check `bf_spool_lag` and for a stale `gc.paused`.

### SpoolLagHigh
Postgres is not keeping up or is down. Check the DB, `bf_dead_lettered_total`, tracker logs for "batch insert failed".

### TrackerShedding
The writer queue is full (`bf_shed_total`). Usually Postgres is slow; see SpoolLagHigh.

### DeadLetterGrowing
Records Postgres rejected for good are skipped. Look for "dead letter" in the tracker log; the offending payload is logged.

### WebhookDeliveryBacklog
`bf_outbox_depth` keeps growing: the worker is down or slow.

### WebhookDeliveryFailing
Customer endpoints are failing; see the deliveries list in the admin UI.
