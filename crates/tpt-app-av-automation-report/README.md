# tpt-app-av-automation-report

Turns execution records into something people and scripts can use, for
[TPT AV Automation](../../README.md).

| Item | Audience | Purpose |
|------|----------|---------|
| `render_record` | People | Human text for one `ExecutionRecord`. The first line says whether it was a **simulation** or a **live** execution, so the two cannot be confused. |
| `machine_result` | Scripts | The stable JSON result for one execution. |
| `exit_code`, `ExitCode` | Shells, CI, schedulers | The stable process exit-code contract. |
| `describe_event` | Both | One-line description of an inbound `Event`. |
| `Filter` | Both | Execution-log filters: rule, status, device, simulated, time range. |

## Exit-code contract

| Code | Name | Meaning |
|-----:|------|---------|
| 0 | `Success` | Every attempted action succeeded. |
| 1 | `PartialFailure` | Some actions succeeded and some failed. |
| 2 | `Failed` | The execution failed. |
| 3 | `Skipped` | Nothing ran: no rule matched, or a condition was not met. |
| 4 | `ConfigurationError` | The pack, device file or arguments were invalid. |
| 5 | `DeviceError` | A device or endpoint could not be reached or opened. |
| 6 | `InternalError` | An internal error or storage failure. |

These values are **stable**: scripts may depend on them. `exit_code(&records)` picks the worst
outcome across a set of records, and a partial failure is never reported as success.

## Example

```rust,ignore
use tpt_app_av_automation_report::{exit_code, machine_result, render_record};

for record in &records {
    println!("{}", render_record(record));
    println!("{}", machine_result(record));
}
std::process::exit(exit_code(&records) as i32);
```

## Licence

Dual-licensed under [MIT](../../LICENSE-MIT) or [Apache-2.0](../../LICENSE-APACHE), at your option.
