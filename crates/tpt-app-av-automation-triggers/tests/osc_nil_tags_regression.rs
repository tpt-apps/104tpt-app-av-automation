//! Regression: a fuzzer-found OSC packet with many payload-less (`N`) tags used to panic.
use tpt_app_av_automation_core::Timestamp;
use tpt_app_av_automation_triggers::{Inbound, InboundLimits};

#[test]
fn osc_many_nil_tags_does_not_panic() {
    let mut d: Vec<u8> = vec![47, 96, 46, 104, 0, 1, 0, 32, 44];
    d.extend(std::iter::repeat(78).take(122));
    d.extend([83, 100, 0, 0, 47, 96, 46, 104, 0, 1, 0, 32, 44, 134, 0]);
    let inbound = Inbound::new(InboundLimits::default());
    let _ = inbound.osc("fuzz", &d, Timestamp::from_millis(0));
}
