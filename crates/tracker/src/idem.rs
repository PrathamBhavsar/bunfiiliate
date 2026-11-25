//! `/v2` idempotency (contract section 5). A bounded in-memory map
//! `(program, source, external id) -> (first event id, body sha256)`, so the handler never
//! reads the DB (anti-pattern 4). Keys live in two generations: when the current one holds
//! `cap` keys it becomes the old one and the previous old one is dropped (O(1) amortized,
//! bounded memory, no scan on the hot path; anti-pattern 28). The DB unique key stays the
//! source of truth after a restart or eviction.

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const DEFAULT_CAP: usize = 50_000;

type Key = (Uuid, &'static str, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pending,
    Committed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: Uuid,
    pub sha: [u8; 32],
    pub state: State,
}

type Val = Entry;

#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// First sighting: the caller owns `id`, is marked `Pending`, and must `commit`
    /// it once the spool write succeeds, or `release` it if ingest fails.
    New,
    /// Same key, same body: answer 200 with the FIRST event's id.
    Duplicate(Uuid),
    /// Same key, different body: answer 409.
    Conflict,
    /// Same key, first attempt still in flight: answer 503 (retry).
    Pending,
}

pub struct Idem {
    cap: usize,
    gens: Mutex<(HashMap<Key, Val>, HashMap<Key, Val>)>,
}

impl Default for Idem {
    fn default() -> Self {
        Self::new(DEFAULT_CAP)
    }
}

/// Holds a `New` claim. Dropping it frees a still-`Pending` key (ingest failed, or the request
/// future was dropped on client disconnect); `release` leaves committed keys alone.
pub struct Reservation {
    idem: std::sync::Arc<Idem>,
    program: Uuid,
    source: &'static str,
    external_id: String,
}

impl Reservation {
    pub fn new(
        idem: std::sync::Arc<Idem>,
        program: Uuid,
        source: &'static str,
        external_id: String,
    ) -> Self {
        Self {
            idem,
            program,
            source,
            external_id,
        }
    }

    pub fn commit(&self) {
        self.idem
            .commit(self.program, self.source, &self.external_id);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.idem
            .release(self.program, self.source, &self.external_id);
    }
}

pub fn digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

impl Idem {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            gens: Mutex::default(),
        }
    }

    /// Look the key up, or reserve it for `id` in one critical section (concurrent
    /// duplicates cannot both win).
    pub fn claim(
        &self,
        program: Uuid,
        source: &'static str,
        external_id: &str,
        sha: [u8; 32],
        id: Uuid,
    ) -> Claim {
        let key = (program, source, external_id.to_owned());
        let mut g = self.gens.lock().unwrap_or_else(|e| e.into_inner());
        let seen = g.0.get(&key).or_else(|| g.1.get(&key)).copied();
        if let Some(entry) = seen {
            if entry.sha != sha {
                return Claim::Conflict;
            }
            return match entry.state {
                State::Pending => Claim::Pending,
                State::Committed => Claim::Duplicate(entry.id),
            };
        }
        if g.0.len() >= self.cap {
            let full = std::mem::take(&mut g.0);
            g.1 = full;
        }
        g.0.insert(
            key,
            Entry {
                id,
                sha,
                state: State::Pending,
            },
        );
        Claim::New
    }

    /// Mark a pending reservation as committed once ingest (spool write) succeeds.
    pub fn commit(&self, program: Uuid, source: &'static str, external_id: &str) {
        let key = (program, source, external_id.to_owned());
        let mut g = self.gens.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = g.0.get_mut(&key) {
            entry.state = State::Committed;
        } else if let Some(entry) = g.1.get_mut(&key) {
            entry.state = State::Committed;
        }
    }

    /// Forget a reservation whose ingest failed (503), so the client's retry is not
    /// answered as a duplicate of an event that was never stored.
    pub fn release(&self, program: Uuid, source: &'static str, external_id: &str) {
        let key = (program, source, external_id.to_owned());
        let mut g = self.gens.lock().unwrap_or_else(|e| e.into_inner());
        if g.0.get(&key).is_some_and(|e| e.state == State::Pending) {
            g.0.remove(&key);
        }
        if g.1.get(&key).is_some_and(|e| e.state == State::Pending) {
            g.1.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_returns_first_id_and_conflict_on_other_body() {
        let i = Idem::new(10);
        let (p, a, b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        assert_eq!(i.claim(p, "generic", "k", digest(b"x"), a), Claim::New);
        // In-flight attempt with same body is Pending
        assert_eq!(i.claim(p, "generic", "k", digest(b"x"), b), Claim::Pending);
        assert_eq!(i.claim(p, "generic", "k", digest(b"y"), b), Claim::Conflict);
        // After commit, returns Duplicate
        i.commit(p, "generic", "k");
        assert_eq!(
            i.claim(p, "generic", "k", digest(b"x"), b),
            Claim::Duplicate(a)
        );
        assert_eq!(i.claim(p, "generic", "k", digest(b"y"), b), Claim::Conflict);
        // other program or source: independent
        assert_eq!(
            i.claim(Uuid::now_v7(), "generic", "k", digest(b"x"), b),
            Claim::New
        );
        assert_eq!(i.claim(p, "stripe", "k", digest(b"x"), b), Claim::New);
    }

    #[test]
    fn in_flight_pending_state_and_failed_release() {
        let i = Idem::new(10);
        let p = Uuid::now_v7();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        assert_eq!(i.claim(p, "generic", "k1", digest(b"data"), a), Claim::New);
        assert_eq!(
            i.claim(p, "generic", "k1", digest(b"data"), b),
            Claim::Pending
        );
        i.release(p, "generic", "k1");
        assert_eq!(i.claim(p, "generic", "k1", digest(b"data"), b), Claim::New);
        i.commit(p, "generic", "k1");
        assert_eq!(
            i.claim(p, "generic", "k1", digest(b"data"), Uuid::now_v7()),
            Claim::Duplicate(b)
        );
    }

    #[test]
    fn dropped_reservation_frees_pending_key_but_not_committed() {
        let i = std::sync::Arc::new(Idem::new(10));
        let p = Uuid::now_v7();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        assert_eq!(i.claim(p, "generic", "k", digest(b"x"), a), Claim::New);
        drop(Reservation::new(i.clone(), p, "generic", "k".into()));
        assert_eq!(i.claim(p, "generic", "k", digest(b"x"), b), Claim::New);
        let r = Reservation::new(i.clone(), p, "generic", "k".into());
        r.commit();
        drop(r);
        assert_eq!(
            i.claim(p, "generic", "k", digest(b"x"), a),
            Claim::Duplicate(b)
        );
    }

    #[test]
    fn release_allows_retry() {
        let i = Idem::new(10);
        let (p, a, b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        i.claim(p, "generic", "k", digest(b"x"), a);
        i.release(p, "generic", "k");
        assert_eq!(i.claim(p, "generic", "k", digest(b"x"), b), Claim::New);
    }

    #[test]
    fn bounded_by_generations() {
        let i = Idem::new(2);
        let p = Uuid::now_v7();
        let first = Uuid::now_v7();
        i.claim(p, "generic", "a", digest(b"x"), first);
        i.commit(p, "generic", "a");
        i.claim(p, "generic", "b", digest(b"x"), Uuid::now_v7());
        i.commit(p, "generic", "b");
        // rotates: a, b move to the old generation but are still remembered
        i.claim(p, "generic", "c", digest(b"x"), Uuid::now_v7());
        i.commit(p, "generic", "c");
        assert_eq!(
            i.claim(p, "generic", "a", digest(b"x"), Uuid::now_v7()),
            Claim::Duplicate(first)
        );
        // two rotations later "a" is forgotten; memory never exceeds 2 * cap
        i.claim(p, "generic", "d", digest(b"x"), Uuid::now_v7());
        i.commit(p, "generic", "d");
        i.claim(p, "generic", "e", digest(b"x"), Uuid::now_v7());
        i.commit(p, "generic", "e");
        i.claim(p, "generic", "f", digest(b"x"), Uuid::now_v7());
        i.commit(p, "generic", "f");
        let g = i.gens.lock().unwrap();
        assert!(g.0.len() + g.1.len() <= 4);
    }
}
