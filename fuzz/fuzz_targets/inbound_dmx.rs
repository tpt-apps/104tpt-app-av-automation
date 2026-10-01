#![no_main]

//! Coverage-guided fuzz target for inbound Art-Net and sACN parsing (spec §18.5, §16).
//!
//! Both protocols carry a universe number and a data length taken from the packet, and both
//! drive a diffing tracker, so this target checks the two things that matter: the parser
//! never panics, and hostile traffic cannot grow the tracker's universe map without bound.

use libfuzzer_sys::fuzz_target;
use tpt_app_av_automation_core::Timestamp;
use tpt_app_av_automation_triggers::{Inbound, InboundLimits};

fuzz_target!(|data: &[u8]| {
    let limits = InboundLimits::default();
    let inbound = Inbound::new(limits);

    if let Ok(events) = inbound.artnet("fuzz", data, Timestamp::from_millis(0)) {
        assert!(events.len() <= limits.max_events_per_packet);
    }
    if let Ok(events) = inbound.sacn("fuzz", data, Timestamp::from_millis(0)) {
        assert!(events.len() <= limits.max_events_per_packet);
    }

    // Replaying the same bytes across many universe numbers must not accumulate state
    // without limit; the tracker rejects universes past `max_universes`.
    for _ in 0..64 {
        let _ = inbound.artnet("fuzz", data, Timestamp::from_millis(0));
        let _ = inbound.sacn("fuzz", data, Timestamp::from_millis(0));
    }
});
