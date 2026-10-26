//! `?page=&limit=` query and the list envelope.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct PageQuery {
    /// 1-based page number (default 1).
    page: Option<u32>,
    /// Items per page (default 50, max 200).
    limit: Option<u32>,
}

impl PageQuery {
    pub fn page(&self) -> u32 {
        self.page.unwrap_or(1).max(1)
    }

    pub fn limit(&self) -> i64 {
        i64::from(self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
    }

    pub fn offset(&self) -> i64 {
        i64::from(self.page() - 1) * self.limit()
    }

    pub fn wrap<T>(&self, (items, total): (Vec<T>, i64)) -> Paged<T> {
        Paged {
            items,
            page: self.page(),
            limit: self.limit(),
            total,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub limit: i64,
    /// Matching rows across all pages.
    pub total: i64,
}
