# tpt-app-av-automation-scenarios

Integration and chaos scenarios for [TPT AV Automation](../../README.md), run against the **shipped
`tpt-av-automation` binary** as a separate process. That is the only way to test what an operator
actually gets: real exit codes, real sockets, a real SQLite state directory and a real forced restart.

This crate has no runtime code of its own beyond test plumbing; it is not meant to be depended on.

## Layout

The suites live in the top-level directories the specification calls for, and are wired in as
`[[test]]` targets so `cargo test --workspace` runs them:

| Target | Source | Covers |
|--------|--------|--------|
| `integration` | `tests/integration/rule_pack_lifecycle.rs` | validate, simulate, run, history and exit codes end to end. |
| `integration_network` | `tests/integration/end_to_end_over_the_wire.rs` | real UDP traffic in, real control messages out, the local API. |
| `chaos_restart` | `tests/chaos/restart_under_load.rs` | killing and restarting the engine under load; state survives. |
| `chaos_misbehaving_peers` | `tests/chaos/misbehaving_peers.rs` | malformed, flooding and silent peers. |

## Helpers (`src/lib.rs`)

`cli` / `cli_bin` (run the binary), `Engine` (a managed long-running instance), `TempDir`,
`reserve_udp_port` / `free_udp_port`, `http_get` / `http_post`, `wait_until`, and the exit-code and
token constants. Everything here is process and filesystem plumbing; the assertions live with each
scenario.

## Running

```sh
cargo build -p tpt-app-av-automation-cli      # scenarios need the binary
cargo test -p tpt-app-av-automation-scenarios
```

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
