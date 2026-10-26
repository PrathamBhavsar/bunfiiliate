//! Country of a click: `CF-IPCountry` first, then an optional DB-IP lite mmdb, else none.

use std::net::IpAddr;
use std::path::Path;

use maxminddb::{MaxMindDbError, Reader, path};

use crate::event::country;

/// Optional country database. `Geo::disabled()` (no database configured) never resolves.
pub struct Geo(Option<Reader<Vec<u8>>>);

impl Geo {
    pub fn disabled() -> Self {
        Self(None)
    }

    pub fn open(db: &Path) -> Result<Self, MaxMindDbError> {
        Reader::open_readfile(db).map(|r| Self(Some(r)))
    }

    pub fn country(&self, header: Option<&str>, ip: Option<&str>) -> Option<String> {
        pick(header, ip, |ip| {
            let result = self.0.as_ref()?.lookup(ip).ok()?;
            let iso: String = result.decode_path(&path!["country", "iso_code"]).ok()??;
            country(&iso)
        })
    }
}

/// The CDN header wins; the database is only asked when it is absent or unusable.
fn pick(
    header: Option<&str>,
    ip: Option<&str>,
    lookup: impl FnOnce(IpAddr) -> Option<String>,
) -> Option<String> {
    header
        .and_then(country)
        .or_else(|| lookup(ip?.parse().ok()?))
}

#[cfg(test)]
mod geo_tests {
    use super::*;

    /// One-node IPv4 tree: addresses below 128.0.0.0 map to `{country: {iso_code: "DE"}}`,
    /// the rest are not found. Hand-built so no third-party database is vendored.
    fn fixture() -> Vec<u8> {
        let s = |s: &str| [&[0x40 | s.len() as u8][..], s.as_bytes()].concat();
        let u16 = |v: u16| [&[0xA2][..], &v.to_be_bytes()].concat();
        let mut db = vec![0, 0, 17, 0, 0, 1]; // left -> data offset 0, right -> no data
        db.extend([0; 16]); // data section separator
        db.push(0xE1);
        db.extend(s("country"));
        db.push(0xE1);
        db.extend(s("iso_code"));
        db.extend(s("DE"));
        db.extend(b"\xAB\xCD\xEFMaxMind.com");
        db.push(0xE9); // metadata map, 9 keys
        for (k, v) in [
            ("binary_format_major_version", u16(2)),
            ("binary_format_minor_version", u16(0)),
            ("build_epoch", vec![0x01, 0x02, 0x00]),
            ("database_type", s("DBIP-Country-Lite")),
            ("description", vec![0xE0]),
            ("ip_version", u16(4)),
            ("languages", vec![0x00, 0x04]),
            ("node_count", vec![0xC4, 0, 0, 0, 1]),
            ("record_size", u16(24)),
        ] {
            db.extend(s(k));
            db.extend(v);
        }
        db
    }

    fn geo() -> Geo {
        Geo(Some(Reader::from_source(fixture()).unwrap()))
    }

    #[test]
    fn header_wins_without_asking_the_database() {
        let got = pick(Some("fr"), Some("1.2.3.4"), |_| {
            panic!("db must not be asked")
        });
        assert_eq!(got.as_deref(), Some("FR"));
    }

    #[test]
    fn unusable_header_falls_back_to_database() {
        let lookup = |_| Some("DE".to_owned());
        for h in ["XX", "T1", "", "usa"] {
            assert_eq!(
                pick(Some(h), Some("1.2.3.4"), lookup).as_deref(),
                Some("DE")
            );
        }
        assert_eq!(pick(None, Some("1.2.3.4"), lookup).as_deref(), Some("DE"));
    }

    #[test]
    fn no_header_and_no_usable_ip_is_none() {
        let lookup = |_| Some("DE".to_owned());
        assert_eq!(pick(None, None, lookup), None);
        assert_eq!(pick(None, Some("not an ip"), lookup), None);
    }

    #[test]
    fn mmdb_lookup() {
        let geo = geo();
        assert_eq!(geo.country(None, Some("1.2.3.4")).as_deref(), Some("DE"));
        assert_eq!(geo.country(None, Some("200.1.1.1")), None);
        assert_eq!(
            geo.country(Some("US"), Some("1.2.3.4")).as_deref(),
            Some("US")
        );
    }

    #[test]
    fn disabled_uses_header_only() {
        let geo = Geo::disabled();
        assert_eq!(
            geo.country(Some("us"), Some("1.2.3.4")).as_deref(),
            Some("US")
        );
        assert_eq!(geo.country(None, Some("1.2.3.4")), None);
    }

    #[test]
    fn open_missing_file_errors() {
        assert!(Geo::open(Path::new("/nonexistent/dbip.mmdb")).is_err());
    }
}
