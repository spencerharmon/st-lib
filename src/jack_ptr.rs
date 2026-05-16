//! Helpers for shuttling a `*mut jack_client_t` (or `*const`) across thread
//! boundaries via its exposed provenance address.
//!
//! The JACK realtime process callback gives us a raw client pointer; we
//! sometimes need to keep using it from non-RT threads (e.g. to call
//! `jack_transport_query`). The pointer itself is not `Send`, so we convert
//! it to a `usize` via `expose_provenance` on the sending side and back via
//! `with_exposed_provenance` on the receiving side.
//!
//! This module wraps that pattern so call-sites don't repeat the unsafe
//! boilerplate or accidentally drift the convention.

use jack::jack_sys as j;

/// Convert a raw JACK client pointer to a thread-Send `usize` address.
///
/// Mirrors `<*mut T>::expose_provenance()` / `<*const T>::expose_provenance()`.
#[inline]
pub fn expose_client(client: *const j::jack_client_t) -> usize {
    client.expose_provenance()
}

/// Recover a `*const jack_client_t` from a previously-exposed address.
///
/// # Safety
/// The caller must guarantee that `addr` was produced by [`expose_client`]
/// for a client that is still alive and valid for the duration of any
/// dereference.
#[inline]
pub unsafe fn recover_client(addr: usize) -> *const j::jack_client_t {
    std::ptr::with_exposed_provenance(addr)
}

/// Recover a `*mut jack_client_t` from a previously-exposed address.
///
/// # Safety
/// Same contract as [`recover_client`], plus the caller must additionally
/// guarantee no aliasing rules are violated by treating the pointer as
/// mutable.
#[inline]
pub unsafe fn recover_client_mut(addr: usize) -> *mut j::jack_client_t {
    std::ptr::with_exposed_provenance_mut(addr)
}
