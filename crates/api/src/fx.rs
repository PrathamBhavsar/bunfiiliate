//! Exchange-rate cache fed by a daily worker tick.
//!
//! Only USD-based rates are fetched and stored (`exchange_rates.base = 'USD'`); the read side
//! triangulates every other pair through USD (`bf_core::convert_cents`). A failed fetch leaves the
//! cache untouched: readers keep using the latest older row, and a missing rate is reported, never
//! replaced by 1:1.

use std::time::Duration;

use bf_core::{Currency, Rate};
use chrono::NaiveDate;
use sqlx::PgPool;

/// Host verified 2026-11-07: `api.frankfurter.app` answers `301` to this one. Fixed first-party
/// host, not user input, so it skips the SSRF guard.
const FRANKFURTER: &str = "https://api.frankfurter.dev/v1/latest";
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FxError {
    /// Network, HTTP status, or the source said no.
    Unavailable(String),
    /// The response was not the documented shape.
    Malformed(String),
    Db(String),
}

impl std::fmt::Display for FxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "rate source unavailable: {m}"),
            Self::Malformed(m) => write!(f, "rate source sent a bad response: {m}"),
            Self::Db(m) => write!(f, "rate cache: {m}"),
        }
    }
}

impl std::error::Error for FxError {}

impl From<sqlx::Error> for FxError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// Units of each currency per 1 USD, as of a date (the source's own date, not ours).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxQuote {
    pub date: NaiveDate,
    pub rates: Vec<(Currency, Rate)>,
}

/// Where rates come from: an enum, not a trait object (like `Source`).
pub enum RateSource {
    Frankfurter(reqwest::Client),
    /// Fixed answer, for tests.
    Mock(Result<FxQuote, FxError>),
}

impl RateSource {
    pub fn frankfurter() -> Self {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("static client config");
        Self::Frankfurter(client)
    }

    /// USD -> each of `symbols`. The source may return fewer.
    pub async fn fetch(&self, symbols: &[Currency]) -> Result<FxQuote, FxError> {
        match self {
            Self::Mock(r) => r.clone(),
            Self::Frankfurter(http) => {
                let list: Vec<&str> = symbols.iter().map(Currency::as_str).collect();
                let res = http
                    // codes are 3 ASCII capitals (`Currency`), so no escaping is needed
                    .get(format!("{FRANKFURTER}?base=USD&symbols={}", list.join(",")))
                    .send()
                    .await
                    .map_err(|e| FxError::Unavailable(e.to_string()))?;
                if !res.status().is_success() {
                    return Err(FxError::Unavailable(format!("HTTP {}", res.status())));
                }
                if res.content_length().is_some_and(|n| n > MAX_BODY as u64) {
                    return Err(FxError::Malformed("body too large".into()));
                }
                let body = res
                    .text()
                    .await
                    .map_err(|e| FxError::Unavailable(e.to_string()))?;
                if body.len() > MAX_BODY {
                    return Err(FxError::Malformed("body too large".into()));
                }
                parse_frankfurter(&body, symbols)
            }
        }
    }
}

/// `{"amount":1.0,"base":"USD","date":"2026-11-06","rates":{"EUR":0.89566}}`. Currencies that
/// were not asked for, or whose rate is not a plain positive decimal, are dropped.
pub fn parse_frankfurter(body: &str, symbols: &[Currency]) -> Result<FxQuote, FxError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| FxError::Malformed(e.to_string()))?;
    if v["base"].as_str() != Some("USD") {
        return Err(FxError::Malformed("base is not USD".into()));
    }
    let date = v["date"]
        .as_str()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .ok_or_else(|| FxError::Malformed("no date".into()))?;
    let obj = v["rates"]
        .as_object()
        .ok_or_else(|| FxError::Malformed("no rates".into()))?;
    let rates = symbols
        .iter()
        .filter_map(|c| {
            let n = obj.get(c.as_str())?.as_number()?;
            Some((*c, Rate::parse(&n.to_string()).ok()?))
        })
        .collect();
    Ok(FxQuote { date, rates })
}

/// Fetch USD -> every non-USD program currency in use (any workspace) and upsert it under the
/// source's date. Returns rows written; 0 without a call when every program is USD. On a failed
/// fetch nothing is written.
pub async fn tick(pool: &PgPool, source: &RateSource) -> Result<u64, FxError> {
    // system loop: currencies in use across all workspaces, no request context
    let codes: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT currency::text FROM programs WHERE currency <> 'USD' ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    let symbols: Vec<Currency> = codes.iter().filter_map(|c| Currency::new(c)).collect();
    if symbols.is_empty() {
        return Ok(0);
    }
    let quote = source.fetch(&symbols).await?;
    let mut written = 0;
    for (cur, rate) in quote.rates.iter().filter(|(c, _)| symbols.contains(c)) {
        let text = format!(
            "{}.{:08}",
            rate.scaled() / 100_000_000,
            rate.scaled() % 100_000_000
        );
        written += sqlx::query(
            "INSERT INTO exchange_rates (base, quote, rate, as_of) \
             VALUES ('USD', $1, $2::numeric, $3) \
             ON CONFLICT (base, quote, as_of) DO UPDATE SET rate = EXCLUDED.rate, fetched_at = now()",
        )
        .bind(cur.as_str())
        .bind(&text)
        .bind(quote.date)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur(c: &str) -> Currency {
        Currency::new(c).unwrap()
    }

    #[test]
    fn parses_the_documented_frankfurter_shape() {
        let body = r#"{"amount":1.0,"base":"USD","date":"2026-11-06","rates":{"EUR":0.89566,"GBP":0.74711,"JPY":151.1}}"#;
        let q = parse_frankfurter(body, &[cur("EUR"), cur("JPY"), cur("CHF")]).unwrap();
        assert_eq!(q.date, NaiveDate::from_ymd_opt(2026, 11, 6).unwrap());
        assert_eq!(
            q.rates,
            [
                (cur("EUR"), Rate::parse("0.89566").unwrap()),
                (cur("JPY"), Rate::parse("151.1").unwrap())
            ]
        );
    }

    #[test]
    fn rejects_other_shapes_and_drops_odd_rates() {
        let s = [cur("EUR")];
        for bad in [
            "",
            "[]",
            r#"{"base":"EUR","date":"2026-11-06","rates":{}}"#,
            r#"{"base":"USD","date":"nope","rates":{}}"#,
            r#"{"base":"USD","date":"2026-11-06"}"#,
        ] {
            assert!(parse_frankfurter(bad, &s).is_err(), "{bad}");
        }
        for odd in [r#""x""#, "0", "-1", "1e-9", "null"] {
            let body = format!(r#"{{"base":"USD","date":"2026-11-06","rates":{{"EUR":{odd}}}}}"#);
            assert_eq!(parse_frankfurter(&body, &s).unwrap().rates, [], "{odd}");
        }
    }
}
