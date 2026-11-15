//! B05: a self-contained HTML invoice for one payout. No JS, no external assets, no templating
//! crate: values go through [`esc`], the only place markup is escaped.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::routing::get;
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::AppState;
use crate::auth::{Any, Auth};
use crate::error::{AppError, ErrorBody};
use crate::programs::viewer;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/payouts/{id}/invoice.html", get(invoice))
}

/// HTML-escape text for element content and quoted attribute values.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `1234.50`, with a leading `-` for negatives.
pub fn amount(cents: i64) -> String {
    let abs = cents.unsigned_abs();
    format!(
        "{}{}.{:02}",
        if cents < 0 { "-" } else { "" },
        abs / 100,
        abs % 100
    )
}

#[derive(FromRow)]
struct Head {
    workspace_name: String,
    affiliate_name: String,
    status: String,
    amount_cents: i64,
    currency: String,
    paid_at: Option<String>,
}

#[derive(FromRow, Serialize)]
struct Line {
    id: Uuid,
    occurred: String,
    amount_cents: i64,
    commission_cents: i64,
}

fn render(payout: Uuid, h: &Head, lines: &[Line]) -> String {
    let cur = esc(&h.currency);
    let mut rows = String::new();
    let mut listed = 0_i64;
    for l in lines {
        listed += l.commission_cents;
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>\n",
            l.id,
            esc(&l.occurred),
            amount(l.amount_cents),
            amount(l.commission_cents)
        ));
    }
    // Clawbacks from reversed paid conversions are netted into the payout; show them.
    let adjustment = h.amount_cents - listed;
    if adjustment != 0 {
        rows.push_str(&format!(
            "<tr><td colspan=\"3\">Adjustments (reversed conversions)</td><td class=\"n\">{}</td></tr>\n",
            amount(adjustment)
        ));
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Invoice {payout}</title>\n<style>\
body{{font:14px/1.5 system-ui,sans-serif;max-width:48rem;margin:2rem auto;padding:0 1rem}}\
table{{border-collapse:collapse;width:100%}}th,td{{border-bottom:1px solid #ccc;padding:.4rem;text-align:left}}\
.n{{text-align:right}}tfoot td{{font-weight:bold}}\
</style></head><body>\n<h1>Payout invoice</h1>\n\
<p><b>From:</b> {aff}<br><b>To:</b> {ws}<br><b>Payout:</b> {payout}<br>\
<b>Status:</b> {status}<br><b>Paid:</b> {paid}</p>\n\
<table><thead><tr><th>Conversion</th><th>Date</th><th class=\"n\">Sale ({cur})</th>\
<th class=\"n\">Commission ({cur})</th></tr></thead>\n<tbody>\n{rows}</tbody>\n\
<tfoot><tr><td colspan=\"3\">Total</td><td class=\"n\" data-total=\"{total}\">{total_text} {cur}</td></tr></tfoot>\
</table>\n</body></html>\n",
        aff = esc(&h.affiliate_name),
        ws = esc(&h.workspace_name),
        status = esc(&h.status),
        paid = h.paid_at.as_deref().map_or("-".to_owned(), esc),
        total = h.amount_cents,
        total_text = amount(h.amount_cents),
    )
}

/// Invoice of an approved or paid payout: the workspace admin, or the affiliate it belongs to.
#[utoipa::path(
    get,
    path = "/api/v1/payouts/{id}/invoice.html",
    tag = "payouts",
    security(("cookie" = [])),
    params(("id" = Uuid, Path, description = "Payout id")),
    responses(
        (status = 200, description = "text/html", content_type = "text/html", body = String),
        (status = 404, description = "Unknown, draft, or someone else's payout", body = ErrorBody),
    )
)]
pub async fn invoice(
    State(s): State<AppState>,
    auth: Auth<Any>,
    Path(id): Path<Uuid>,
) -> Result<([(axum::http::HeaderName, &'static str); 3], String), AppError> {
    let (ws, own) = viewer(&s, &auth.principal).await?;
    let head: Head = sqlx::query_as(
        "SELECT w.name AS workspace_name, \
                COALESCE(a.tax_legal_name, a.payout_email::text, u.email::text) AS affiliate_name, \
                p.status::text AS status, p.amount_cents, pr.currency::text AS currency, \
                to_char(b.paid_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS paid_at \
         FROM payouts p \
         JOIN payout_batches b ON b.id = p.batch_id \
         JOIN programs pr ON pr.id = b.program_id \
         JOIN workspaces w ON w.id = pr.workspace_id \
         JOIN affiliates a ON a.id = p.affiliate_id \
         JOIN users u ON u.id = a.user_id \
         WHERE p.id = $1 AND pr.workspace_id = $2 AND a.workspace_id = $2 \
           AND p.status IN ('approved', 'paid') AND ($3::uuid IS NULL OR p.affiliate_id = $3)",
    )
    .bind(id)
    .bind(ws)
    .bind(own)
    .fetch_optional(&s.pool)
    .await?
    .ok_or(AppError::NotFound("payout not found"))?;
    let lines: Vec<Line> = sqlx::query_as(
        "SELECT c.id, to_char(c.occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS occurred, \
                c.amount_cents, c.commission_cents \
         FROM conversions c JOIN programs pr ON pr.id = c.program_id \
         WHERE c.payout_id = $1 AND pr.workspace_id = $2 ORDER BY c.occurred_at, c.id",
    )
    .bind(id)
    .bind(ws)
    .fetch_all(&s.pool)
    .await?;
    Ok((
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'",
            ),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        render(id, &head, &lines),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_covers_markup_and_quotes() {
        assert_eq!(
            esc("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
        assert_eq!(esc("plain é"), "plain é");
    }

    #[test]
    fn amount_formats_cents() {
        assert_eq!(amount(0), "0.00");
        assert_eq!(amount(5), "0.05");
        assert_eq!(amount(123_456), "1234.56");
        assert_eq!(amount(-5), "-0.05");
        assert_eq!(amount(i64::MIN), "-92233720368547758.08");
    }
}
