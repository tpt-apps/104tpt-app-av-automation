#![no_main]

//! Coverage-guided fuzz target for inbound OSC packet parsing (spec §18.5, §16).
//!
//! OSC arrives over UDP from control surfaces on the venue network, so it is untrusted.
//! The invariant is that arbitrary bytes are either rejected or produce a bounded list of
//! well-formed events — never a panic, and never more events than the configured limit.

use libfuzzer_sys::fuzz_target;
use tpt_app_av_automation_core::Timestamp;
use tpt_app_av_automation_triggers::{Inbound, InboundLimits};

fuzz_target!(|data: &[u8]| {
    let limits = InboundLimits::default();
    let inbound = Inbound::new(limits);

    if let Ok(events) = inbound.osc("fuzz", data, Timestamp::from_millis(0)) {
        assert!(
            events.len() <= limits.max_events_per_packet,
            "a single OSC packet expanded to {} events, above the {} limit",
            events.len(),
            limits.max_events_per_packet
        );
        // Rate limiting is the other half of §16: the same source cannot exceed its budget.
        let mut budget = limits.rate_capacity;
        for _ in 0..(limits.rate_capacity + 10) {
            let produced = match inbound.osc("fuzz", data, Timestamp::from_millis(0)) {
                Ok(events) => events.len(),
                Err(_) => 0,
            };
            if produced > 0 {
                budget = budget.saturating_sub(1);
                assert!(budget > 0, "rate limiter allowed more than its capacity");
            }
        }
    }
});
