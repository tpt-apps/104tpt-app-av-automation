//! Measured latency under live-event-like load (spec §18, §24.1 step 20).
//!
//! The throughput test in `chaos.rs` answers "can it keep up?". These tests answer the question an
//! operator cares about: **how long does a cue take, and how bad does it get when the pack is
//! large?** Every figure is a percentile, not an average, because an average hides exactly the
//! tail that makes a show miss a cue.
//!
//! Run them against an optimized build; the assertions are the same either way.
//!
//! ```sh
//! cargo test --release -p tpt-app-av-automation-test --test latency -- --nocapture
//! ```

use std::collections::BTreeMap;
use std::net::UdpSocket;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tpt_app_av_automation_actions::{Actions, MemorySink, NullSleeper};
use tpt_app_av_automation_core::{Clock, Event, FixedClock, SystemClock};
use tpt_app_av_automation_devices::{
    Device, DeviceRegistry, UdpEndpoint, UdpProtocol, VirtualEndpoint,
};
use tpt_app_av_automation_engine::{Engine, EngineConfig};
use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_test::LatencySamples;
use tpt_app_av_automation_triggers::{Inbound, InboundLimits};
use tpt_av_control_osc::{OscArg, OscMessage};

/// A live cue has to land inside a frame budget; the engine's own work must be far inside it again,
/// so that a busy pack still leaves room for the socket writes either side of it.
const CUE_BUDGET_MS: u64 = 20;

/// Events driven through the engine in each in-memory measurement.
const EVENTS: usize = 4_000;

/// A pack of `rules` OSC-triggered rules, each writing to `dev` and alerting.
fn pack(rules: usize) -> String {
    let mut yaml = String::from("format_version: 1\nname: Latency\nrevision: 1\nrules:\n");
    for i in 0..rules {
        yaml.push_str(&format!(
            "  - id: r{i}\n    name: R{i}\n    armed: true\n    trigger: {{ type: osc, address: /load/{i} }}\n    actions:\n      - {{ id: a, type: control.osc, device: dev, address: /out/{i}, args: [1] }}\n      - {{ id: n, type: notify.operator, message: x }}\n"
        ));
    }
    yaml
}

/// An engine over `rules` rules, armed and live, writing to an in-memory endpoint.
fn engine(rules: usize, history_limit: usize) -> Engine {
    let sink = Arc::new(MemorySink::default());
    let mut actions = Actions::new(sink.clone(), sink, Arc::new(NullSleeper));
    actions.bind_endpoint("dev", Arc::new(VirtualEndpoint::new()));
    let mut registry = DeviceRegistry::new();
    registry.register(Device::new("dev", "dev"));
    Engine::new(
        RulePack::from_yaml_str(&pack(rules)).expect("pack parses"),
        registry,
        actions,
        Arc::new(FixedClock::default()),
        EngineConfig {
            history_limit,
            ..EngineConfig::default()
        },
    )
    .expect("engine builds")
}

/// Feeds `EVENTS` OSC events, timing each dispatch, and returns the samples and the number of rules
/// that fired.
///
/// Half the events match a rule (every other address among the first `2 * rules` addresses), so
/// both the matching and the action-dispatch paths are exercised.
fn measure(rules: usize) -> (LatencySamples, usize) {
    let mut engine = engine(rules, 1_000);
    let mut samples = LatencySamples::new();
    let mut fired = 0usize;
    for i in 0..EVENTS {
        let event = Event::Osc {
            address: format!("/load/{}", i % (rules * 2)),
            args: vec![],
        };
        let started = Instant::now();
        fired += engine.handle_event(event).len();
        samples.record(started.elapsed());
    }
    assert_eq!(samples.len(), EVENTS);
    (samples, fired)
}

/// A typical live pack: dispatch is dominated by work the engine does, and its worst case stays far
/// inside a frame budget.
#[test]
fn a_twenty_rule_pack_dispatches_every_cue_well_inside_a_frame_budget() {
    let (samples, fired) = measure(20);
    let p = samples.percentiles().expect("samples were recorded");
    eprintln!("engine dispatch, 20 rules: {p}");
    assert_eq!(
        fired,
        EVENTS / 2,
        "half the events address one of the 20 rules"
    );
    // The gate is on the tail, not the median: one slow dispatch is a visible glitch.
    assert!(
        p.p99 < CUE_BUDGET_MS * 1_000,
        "p99 dispatch latency was {} us, over the {CUE_BUDGET_MS} ms cue budget",
        p.p99
    );
}

/// A cue that matches nothing must be cheaper than one that fires a chain: the engine should reject
/// it during matching without entering action execution at all.
#[test]
fn an_unmatched_event_is_cheaper_than_a_fired_one() {
    let mut engine = engine(20, 1_000);
    let mut unmatched = LatencySamples::new();
    let mut matched = LatencySamples::new();
    for _ in 0..EVENTS {
        for (address, is_match) in [("/load/nothing-here", false), ("/load/3", true)] {
            let started = Instant::now();
            let records = engine.handle_event(Event::Osc {
                address: address.to_string(),
                args: vec![],
            });
            let elapsed = started.elapsed();
            if is_match {
                assert_eq!(records.len(), 1);
                matched.record(elapsed);
            } else {
                assert!(records.is_empty());
                unmatched.record(elapsed);
            }
        }
    }
    let u = unmatched.percentiles().expect("unmatched samples");
    let m = matched.percentiles().expect("matched samples");
    eprintln!("20 rules: unmatched {u}");
    eprintln!("20 rules: matched   {m}");
    assert!(
        u.max <= m.max.max(1),
        "an event that matches nothing took longer ({} us) than one that runs a chain ({} us)",
        u.max,
        m.max
    );
}

/// Pack size is the operator-controlled cost: a venue with more rules should not see the tail grow
/// without bound. This is the scaling profile — the number that says whether the rule scan needs
/// work — and every size is printed so the curve is on the record, not just in a threshold.
#[test]
fn dispatch_latency_scales_with_pack_size() {
    let mut previous_p50 = 0u64;
    for rules in [5usize, 20, 100, 500] {
        let (samples, _) = measure(rules);
        let p = samples.percentiles().expect("samples were recorded");
        eprintln!("{rules:>4} rules: {p}");
        assert!(
            p.p99 < CUE_BUDGET_MS * 1_000,
            "{rules} rules: p99 dispatch latency {} us is over the {CUE_BUDGET_MS} ms budget",
            p.p99
        );
        if rules > 5 {
            // 500 rules scanned linearly is still microseconds, so the median may not regress by
            // more than two orders of magnitude per 10x of rules. Beyond that it is a complexity
            // change in matching, not pack growth.
            assert!(
                p.p50 < previous_p50.saturating_mul(100).max(1_000),
                "{rules} rules: median {} us is more than 100x the {} us of the previous size",
                p.p50,
                previous_p50
            );
        }
        previous_p50 = p.p50;
    }
}

/// The tests above measure the engine with an in-memory device. This one measures what an operator
/// actually waits on: a real OSC datagram arriving on a socket, parsed, matched, executed, and
/// leaving on a *second* real socket as the cue the fixture would receive.
///
/// ```text
///   sender ──OSC in──▶ listener ──▶ Inbound ──▶ Engine ──▶ UdpEndpoint ──OSC out──▶ fixture
///      │                                                                              ▲
///      └───────────────────────── timed round trip ──────────────────────────────────┘
/// ```
///
/// It cannot measure a physical fixture (CI has no hardware), but it does measure everything the
/// software controls, including the encode and the syscalls the in-memory tests skip.
#[test]
fn an_osc_cue_reaches_a_real_socket_well_inside_the_budget() {
    // The fixture side: a real socket playing the part of the device.
    let fixture = UdpSocket::bind("127.0.0.1:0").expect("bind fixture socket");
    fixture
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fixture read timeout");
    let fixture_addr = fixture.local_addr().expect("fixture addr").to_string();

    let endpoint = UdpEndpoint::new(UdpProtocol::Osc, &fixture_addr, BTreeMap::new())
        .expect("outbound endpoint connects");
    let sink = Arc::new(MemorySink::default());
    let mut actions = Actions::new(sink.clone(), sink, Arc::new(NullSleeper));
    actions.bind_endpoint("dev", Arc::new(endpoint));
    let mut registry = DeviceRegistry::new();
    registry.register(Device::new("dev", "dev"));
    let yaml = "format_version: 1\nname: Wire\nrevision: 1\nrules:\n  - id: cue\n    name: Cue\n    armed: true\n    trigger: { type: osc, address: /go }\n    actions:\n      - { id: a, type: control.osc, device: dev, address: /out, args: [1] }\n";
    let engine = Engine::new(
        RulePack::from_yaml_str(yaml).expect("pack parses"),
        registry,
        actions,
        Arc::new(SystemClock),
        EngineConfig::default(),
    )
    .expect("engine builds");

    // A generous rate budget: this test is about latency, and a source that gets muted would
    // silently measure nothing at all.
    let inbound = Inbound::new(InboundLimits {
        rate_capacity: 1_000_000,
        ..InboundLimits::default()
    });
    let clock = SystemClock;
    let cue = OscMessage::new("/go", &[]).expect("cue builds").encode();

    run_round_trips(engine, inbound, clock, cue, fixture)
}
/// Drives `rounds` cues through a live listener thread and back out of the fixture socket, timing
/// the whole trip and asserting that every cue arrived intact and in order.
fn run_round_trips(
    mut engine: Engine,
    inbound: Inbound,
    clock: SystemClock,
    cue: Vec<u8>,
    fixture: UdpSocket,
) {
    use std::sync::atomic::{AtomicBool, Ordering};

    // The listener runs on its own thread, as it does in production.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("bind listener");
    let listener_addr = listener.local_addr().expect("listener addr");
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let handle: JoinHandle<u64> = std::thread::spawn(move || {
        listener
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("listener read timeout");
        let mut buf = [0u8; 4096];
        let mut handled = 0u64;
        while !stop_thread.load(Ordering::SeqCst) {
            // A read timeout is how the thread notices it should stop.
            let n = match listener.recv(&mut buf) {
                Ok(n) => n,
                Err(_) => continue,
            };
            let events = inbound
                .osc("bench", &buf[..n], clock.now())
                .expect("datagram parses");
            for event in events {
                let records = engine.handle_event(event);
                assert_eq!(records.len(), 1, "the cue rule fired for every datagram");
                handled += 1;
            }
        }
        handled
    });

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    let rounds = 200usize;
    let mut samples = LatencySamples::new();
    let mut buf = [0u8; 1024];
    for round in 0..rounds {
        let started = Instant::now();
        sender.send_to(&cue, listener_addr).expect("cue leaves");
        let n = fixture.recv(&mut buf).expect("fixture receives the cue");
        samples.record(started.elapsed());
        let message = OscMessage::decode(&buf[..n]).expect("outbound datagram is valid OSC");
        assert_eq!(message.address, "/out");
        assert_eq!(message.arguments, vec![OscArg::Int(1)], "round {round}");
    }

    stop.store(true, Ordering::SeqCst);
    let handled = handle.join().expect("listener thread joins");
    assert_eq!(handled, rounds as u64, "every cue was handled exactly once");

    let p = samples.percentiles().expect("samples were recorded");
    eprintln!("datagram in -> datagram out over loopback: {p}");
    assert_eq!(samples.len(), rounds);
    assert!(
        p.p99 < CUE_BUDGET_MS * 1_000,
        "p99 loopback cue latency was {} us, over the {CUE_BUDGET_MS} ms budget",
        p.p99
    );
}
