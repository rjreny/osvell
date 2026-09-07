//! Total-order helpers for f32 sorts.
//!
//! `partial_cmp(...).unwrap_or(Equal)` is not a total order when NaN appears —
//! Rust's sort will panic. Prefer these helpers (or `f32::total_cmp`) everywhere.

use std::cmp::Ordering;

#[inline]
pub fn cmp_f32(a: f32, b: f32) -> Ordering {
    a.total_cmp(&b)
}

#[inline]
pub fn cmp_f32_desc(a: f32, b: f32) -> Ordering {
    b.total_cmp(&a)
}
