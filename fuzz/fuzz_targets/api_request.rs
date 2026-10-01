#![no_main]

//! Coverage-guided fuzz target for local API request parsing (spec §18.5, §14).
//!
//! The API is loopback-only but token-guarded, so it is still the most attacker-reachable
//! surface in the product. The invariant is that a malformed request is rejected cleanly
//! and that the parser never panics, never over-reads, and never reads without bound.

use libfuzzer_sys::fuzz_target;
use tpt_app_av_automation_service::api;

fuzz_target!(|data: &[u8]| {
    let Ok(request) = String::from_utf8(data.to_vec()) else {
        return;
    };
    let mut reader = std::io::BufReader::new(request.as_bytes());

    // `parse_request` must return rather than panic on every possible request line.
    let parsed = api::parse_request(&mut reader);

    // A request that parsed must have a method and a path; anything else is a bug that
    // would let a malformed request reach routing.
    if let Ok(request) = parsed {
        assert!(
            !request.method.is_empty(),
            "a parsed request had an empty method"
        );
        assert!(
            request.path.starts_with('/'),
            "a parsed request had a non-absolute path: {:?}",
            request.path
        );
    }
});
