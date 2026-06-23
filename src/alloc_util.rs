//! Allocation helpers honoring the [`AllocPreference`] policy per call site.
//!
//! A JXL decode mixes two allocation regimes:
//!
//! * **Big, untrusted-sized buffers** (the full-image / per-frame output pixel
//!   buffer, sized from the decoded header dimensions) default to the
//!   *fallible* `try_reserve` path — a malicious header can demand gigabytes,
//!   so we want a graceful [`JxlError::LimitExceeded`] rather than an abort.
//! * **Small, bounded scratch** (when present) defaults to the *infallible*
//!   `vec!` path — a single `calloc` is faster and the size is bounded, not
//!   attacker-controlled in any unbounded way.
//!
//! [`AllocPreference`] is a **3-mode, per-site override** of that default:
//! `Fallible` / `Infallible` force one path everywhere; `CodecDefault` (and any
//! future `#[non_exhaustive]` variant) keeps each site's own default. The helper
//! signatures therefore take the caller's preference *and* the site default, and
//! resolve them together.
//!
//! The heavy VarDCT / modular pass buffers are allocated inside the
//! `zenjxl-decoder` dependency, which this preference cannot reach; it governs
//! only the wrapper-owned output buffers (see `decode.rs` / `codec.rs`).
//!
//! [`AllocPreference`]: zencodec::AllocPreference

use alloc::vec;
use alloc::vec::Vec;
use whereat::{At, at};

use crate::error::JxlError;

/// Resolve the 3-mode [`AllocPreference`](zencodec::AllocPreference) against
/// THIS site's default fallibility.
///
/// * [`Fallible`](zencodec::AllocPreference::Fallible) → always `true`.
/// * [`Infallible`](zencodec::AllocPreference::Infallible) → always `false`.
/// * [`CodecDefault`](zencodec::AllocPreference::CodecDefault) (and any future
///   `#[non_exhaustive]` variant) → the site default, unchanged.
#[inline]
#[must_use]
pub(crate) fn resolve_fallible(
    pref: zencodec::AllocPreference,
    site_default_fallible: bool,
) -> bool {
    match pref {
        zencodec::AllocPreference::Fallible => true,
        zencodec::AllocPreference::Infallible => false,
        _ => site_default_fallible,
    }
}

/// Allocate `n` zeroed bytes, honoring the per-site fallibility.
///
/// `pref` is the caller's [`AllocPreference`](zencodec::AllocPreference);
/// `site_default_fallible` is this site's default when `pref` is `CodecDefault`.
///
/// * fallible → `try_reserve_exact` then zero-fill, returning
///   [`JxlError::LimitExceeded`] on allocation failure.
/// * infallible → `vec![0u8; n]` (single `calloc`, aborts on OOM).
pub(crate) fn alloc_zeroed(
    pref: zencodec::AllocPreference,
    site_default_fallible: bool,
    n: usize,
) -> Result<Vec<u8>, At<JxlError>> {
    if resolve_fallible(pref, site_default_fallible) {
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| {
            at!(JxlError::LimitExceeded(alloc::format!(
                "out of memory allocating {n} bytes"
            )))
        })?;
        v.resize(n, 0);
        Ok(v)
    } else {
        Ok(vec![0u8; n])
    }
}

/// Allocate an empty `Vec<u8>` with reserved capacity for `cap` bytes, honoring
/// the per-site fallibility (for the `Vec::with_capacity` + extend sites).
///
/// `pref` is the caller's [`AllocPreference`](zencodec::AllocPreference);
/// `site_default_fallible` is this site's default when `pref` is `CodecDefault`.
///
/// * fallible → `try_reserve_exact`, returning [`JxlError::LimitExceeded`] on
///   allocation failure.
/// * infallible → `Vec::with_capacity(cap)` (aborts on OOM).
///
/// The returned `Vec` is empty (length 0); the caller fills it.
///
/// Currently unused at the wrapper decode level (zenjxl's untrusted decode
/// allocations are all zeroed output buffers handled by [`alloc_zeroed`]); kept
/// alongside it so future `Vec::with_capacity` + extend decode sites can honor
/// the same preference.
#[cfg(test)]
pub(crate) fn vec_with_capacity(
    pref: zencodec::AllocPreference,
    site_default_fallible: bool,
    cap: usize,
) -> Result<Vec<u8>, At<JxlError>> {
    if resolve_fallible(pref, site_default_fallible) {
        let mut v = Vec::new();
        v.try_reserve_exact(cap).map_err(|_| {
            at!(JxlError::LimitExceeded(alloc::format!(
                "out of memory allocating {cap} bytes"
            )))
        })?;
        Ok(v)
    } else {
        Ok(Vec::with_capacity(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zencodec::AllocPreference;

    // `CodecDefault` keeps each site's own default fallibility.

    #[test]
    fn codec_default_keeps_site_default_true() {
        // Big-buffer site (default fallible): CodecDefault stays fallible.
        assert!(resolve_fallible(AllocPreference::CodecDefault, true));
    }

    #[test]
    fn codec_default_keeps_site_default_false() {
        // Small-scratch site (default infallible): CodecDefault stays infallible.
        assert!(!resolve_fallible(AllocPreference::CodecDefault, false));
    }

    #[test]
    fn explicit_fallible_overrides_any_site_default() {
        assert!(resolve_fallible(AllocPreference::Fallible, false));
        assert!(resolve_fallible(AllocPreference::Fallible, true));
    }

    #[test]
    fn explicit_infallible_overrides_any_site_default() {
        assert!(!resolve_fallible(AllocPreference::Infallible, true));
        assert!(!resolve_fallible(AllocPreference::Infallible, false));
    }

    #[test]
    fn alloc_zeroed_all_modes_equal_bytes() {
        let a = alloc_zeroed(AllocPreference::CodecDefault, true, 4096).unwrap();
        let b = alloc_zeroed(AllocPreference::Infallible, true, 4096).unwrap();
        let c = alloc_zeroed(AllocPreference::Fallible, false, 4096).unwrap();
        assert_eq!(a.len(), 4096);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!(a.iter().all(|&x| x == 0));
    }

    #[test]
    fn vec_with_capacity_reserves_and_is_empty() {
        let a = vec_with_capacity(AllocPreference::Infallible, false, 1024).unwrap();
        let b = vec_with_capacity(AllocPreference::Fallible, false, 1024).unwrap();
        assert_eq!(a.len(), 0);
        assert_eq!(b.len(), 0);
        assert!(a.capacity() >= 1024);
        assert!(b.capacity() >= 1024);
    }

    #[test]
    fn alloc_zeroed_fallible_oom_returns_err() {
        // Request an impossibly large allocation; the fallible path must
        // return Err (mapped to LimitExceeded) rather than abort.
        let r = alloc_zeroed(AllocPreference::Fallible, true, usize::MAX / 2);
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err().error(), JxlError::LimitExceeded(_)));
    }

    #[test]
    fn vec_with_capacity_fallible_oom_returns_err() {
        let r = vec_with_capacity(AllocPreference::Fallible, true, usize::MAX / 2);
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err().error(), JxlError::LimitExceeded(_)));
    }
}
