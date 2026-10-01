#![no_main]

//! Coverage-guided fuzz target for inbound MIDI parsing (spec §18.5, §16).
//!
//! MIDI is the parser with the tightest length arithmetic (status, data and optional
//! running-status bytes), so it is the most likely place for an out-of-bounds read. The
//! invariant is that arbitrary bytes are rejected or decoded, and never panic.

use libfuzzer_sys::fuzz_target;
use tpt_app_av_automation_core::Timestamp;
use tpt_app_av_automation_triggers::{Inbound, InboundLimits};

fuzz_target!(|data: &[u8]| {
    let limits = InboundLimits::default();
    let inbound = Inbound::new(limits);

    if let Ok(events) = inbound.midi("fuzz", data, Timestamp::from_millis(0)) {
        assert!(
            events.len() <= limits.max_events_per_packet,
            "a single MIDI message expanded to {} events, above the {} limit",
            events.len(),
            limits.max_events_per_packet
        );
    }
});
