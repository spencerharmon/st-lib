//! Pure beat-frame arithmetic shared between st-conductor (producer) and
//! st-click / st-loop (consumers).
//!
//! These functions encode the suite's convention for translating between
//! musical time (bar, beat) and audio time (sample frames). They were
//! originally defined inline in =st-conductor/src/rolling.rs= and partially
//! re-implemented in =st-click/src/sequencer.rs=; consolidating here ensures
//! the "*2 half-beat" convention is defined in exactly one place.

/// Frames-per-beat using the conductor's convention.
///
/// NOTE: The formula multiplies by 2, treating one beat as two "half-beats"
/// in the frame accounting. This is what feeds the next-beat-frame calculation
/// and must match across producer and consumers.
#[inline]
pub fn frames_per_beat(frame_rate: u32, tempo: f64) -> f64 {
    let frames_per_minute = frame_rate * 60;
    (frames_per_minute as f64 / tempo) * 2f64
}

/// Given an absolute beat index and frames-per-beat (as computed by
/// [`frames_per_beat`]), return the frame number of the *next* beat.
#[inline]
pub fn next_beat_frame(absolute_beat: u64, frames_per_beat: f64) -> u64 {
    let this_beat_frame = absolute_beat * frames_per_beat as u64;
    this_beat_frame + frames_per_beat as u64
}

/// Convert (bar, beat) and beats-per-bar into an absolute beat index.
/// Bar is 1-indexed.
#[inline]
pub fn absolute_beat(bar: i32, beat: i32, beats_per_bar: f32) -> u64 {
    (beats_per_bar as u64 * (bar as u64 - 1)) + beat as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_per_beat_120bpm_48k() {
        assert_eq!(frames_per_beat(48000, 120.0) as u64, 48000);
    }

    #[test]
    fn frames_per_beat_60bpm_44100() {
        assert_eq!(frames_per_beat(44100, 60.0) as u64, 88200);
    }

    #[test]
    fn absolute_beat_44() {
        assert_eq!(absolute_beat(1, 1, 4.0), 1);
        assert_eq!(absolute_beat(2, 1, 4.0), 5);
        assert_eq!(absolute_beat(3, 2, 4.0), 10);
    }

    #[test]
    fn next_beat_frame_basic() {
        let fpb = frames_per_beat(48000, 120.0);
        assert_eq!(next_beat_frame(4, fpb), 5 * 48000);
    }
}
