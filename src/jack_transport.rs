//! Thin wrapper around `jack_transport_query` that returns a typed snapshot
//! of the fields the st-suite actually reads.
//!
//! The raw FFI dance (`MaybeUninit::uninit().as_mut_ptr() → jack_transport_query
//! → field reads`) was repeated in st-conductor, st-click, and st-loop with
//! slightly different field selections. This module consolidates it.

use jack::jack_sys as j;
use std::mem::MaybeUninit;

/// Snapshot of the JACK transport position fields the suite cares about.
///
/// All fields come straight from `jack_position_t`; see the JACK headers for
/// authoritative semantics. The `state` is the value returned by
/// `jack_transport_query` itself.
#[derive(Debug, Clone, Copy)]
pub struct TransportSnapshot {
    pub state: j::jack_transport_state_t,
    /// Current sample frame.
    pub frame: u64,
    /// Sample rate in frames per second.
    pub frame_rate: u32,
    /// Beats per bar (numerator of the time signature, as a float).
    pub beats_per_bar: f32,
    /// 1-indexed bar number within the current transport position.
    pub bar: i32,
    /// 1-indexed beat number within the current bar.
    pub beat: i32,
    /// Tick within the current beat (0..ticks_per_beat).
    pub tick: i32,
    /// Tempo in beats per minute.
    pub beats_per_minute: f64,
}

/// Query the JACK transport for the given client pointer.
///
/// # Safety
/// `client` must be a valid, currently-alive `*const jack_client_t`. The
/// usual JACK threading rules apply (this function may be called from any
/// thread; it does not need to be RT-safe).
pub unsafe fn query_transport(client: *const j::jack_client_t) -> TransportSnapshot {
    let mut pos: MaybeUninit<j::jack_position_t> = MaybeUninit::uninit();
    let state = j::jack_transport_query(client, pos.as_mut_ptr());
    let p = pos.assume_init();
    TransportSnapshot {
        state,
        frame: p.frame as u64,
        frame_rate: p.frame_rate,
        beats_per_bar: p.beats_per_bar,
        bar: p.bar,
        beat: p.beat,
        tick: p.tick,
        beats_per_minute: p.beats_per_minute,
    }
}
