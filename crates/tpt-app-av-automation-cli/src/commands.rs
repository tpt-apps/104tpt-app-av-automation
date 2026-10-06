//! Command implementations.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tpt_app_av_automation_actions::Actions;
use tpt_app_av_automation_core::{
    Clock, DeviceHealth, Diagnostic, Error, Event, FixedClock, Result, Severity, SystemClock,
    Timestamp,
};
use tpt_app_av_automation_devices::{Device, DeviceFile, DeviceRegistry};
use tpt_app_av_automation_engine::{Engine, EngineConfig, Mode};
use tpt_app_av_automation_model::{
    has_errors, ExecutionRecord, ExecutionStatus, RulePack, Weekday,
};
use tpt_app_av_automation_report::{exit_code, machine_result, render_record, ExitCode, Filter};
use tpt_app_av_automation_service::{
    supervise, Outcome, Service, ServiceConfig, Store, WatchdogEvent, WatchdogPolicy,
};
use tpt_app_av_automation_triggers::LocalClock;

use crate::event_spec::{self, ScheduleAt};
use crate::{DevicesArgs, Format, HistoryArgs, RunArgs, SimulateArgs, ValidateArgs, WatchdogArgs};

/// Monday 2024-01-01 00:00 UTC, the anchor for a simulated clock.
const MONDAY_EPOCH_MS: u64 = 1_704_067_200_000;

fn print_diagnostics(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        eprintln!("  {d}");
    }
}

// --- devices ------------------------------------------------------------------------------------

pub fn devices(args: &DevicesArgs) -> Result<ExitCode> {
    let file = DeviceFile::from_path(&args.devices)?;
    let diagnostics = file.validate();
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();

    // One line per device; with --ping, the endpoint is built and probed.
    let mut rows = Vec::new();
    let mut unreachable = 0;
    for device in &file.devices {
        let status = if !args.ping {
            None
        } else {
            match tpt_app_av_automation_devices::build_endpoint(device) {
                Ok(endpoint) => match endpoint.ping() {
                    Ok(()) => Some(Ok(())),
                    Err(e) => Some(Err(e.to_string())),
                },
                Err(e) => Some(Err(e.to_string())),
            }
        };
        if matches!(status, Some(Err(_))) {
            unreachable += 1;
        }
        rows.push((device, status));
    }

    match args.format {
        Format::Text => {
            print_diagnostics(&diagnostics);
            for (device, status) in &rows {
                let note = match status {
                    None => String::new(),
                    Some(Ok(())) => "  ok".to_string(),
                    Some(Err(e)) => format!("  FAILED: {e}"),
                };
                println!(
                    "{:<20} {:<8} {:<10} {}{note}",
                    device.id,
                    device.protocol,
                    format!("{:?}", device.kind).to_lowercase(),
                    device.address.as_deref().unwrap_or("-"),
                );
            }
            println!(
                "{} device(s), {errors} error(s){}",
                file.devices.len(),
                if args.ping {
                    format!(", {unreachable} failed to open")
                } else {
                    String::new()
                }
            );
        }
        Format::Json | Format::Trace => {
            let devices: Vec<Value> = rows
                .iter()
                .map(|(d, status)| {
                    json!({
                        "id": d.id,
                        "protocol": d.protocol,
                        "address": d.address,
                        "ping": status.as_ref().map(|s| match s {
                            Ok(()) => json!("ok"),
                            Err(e) => json!(e),
                        }),
                    })
                })
                .collect();
            println!(
                "{}",
                json!({ "valid": errors == 0, "devices": devices, "diagnostics": diagnostics })
            );
        }
    }
    Ok(if errors > 0 {
        ExitCode::ConfigurationError
    } else if unreachable > 0 {
        ExitCode::DeviceError
    } else {
        ExitCode::Success
    })
}

// --- validate -----------------------------------------------------------------------------------

pub fn validate(args: &ValidateArgs) -> Result<ExitCode> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    let pack = match RulePack::from_path(&args.rules) {
        Ok(pack) => Some(pack),
        Err(Error::Validation(d)) => {
            diagnostics.extend(d);
            None
        }
        Err(other) => return Err(other),
    };
    if let Some(pack) = &pack {
        // `from_path` already enforced the errors; this surfaces the warnings too.
        diagnostics.extend(pack.validate()?);
    }

    let devices = match &args.devices {
        Some(path) => {
            let file = DeviceFile::from_path(path)?;
            diagnostics.extend(file.validate());
            Some(file)
        }
        None => None,
    };
    if let (Some(pack), Some(devices)) = (&pack, &devices) {
        diagnostics.extend(unresolved_devices(pack, devices));
    }
    if let Some(path) = &args.config {
        diagnostics.extend(ServiceConfig::from_path(path)?.validate());
    }

    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics.len() - errors;
    match args.format {
        Format::Text => {
            print_diagnostics(&diagnostics);
            match &pack {
                Some(pack) if errors == 0 => {
                    let s = pack.summary();
                    println!(
                        "OK: \"{}\" revision {} — {} rule(s) ({} armed, {} disarmed), {} warning(s)",
                        pack.name, pack.revision, s.rules, s.armed, s.disarmed, warnings
                    );
                }
                _ => println!("INVALID: {errors} error(s), {warnings} warning(s)"),
            }
        }
        Format::Json | Format::Trace => println!(
            "{}",
            json!({
                "valid": errors == 0,
                "errors": errors,
                "warnings": warnings,
                "diagnostics": diagnostics,
            })
        ),
    }
    Ok(if errors == 0 {
        ExitCode::Success
    } else {
        ExitCode::ConfigurationError
    })
}

fn unresolved_devices(pack: &RulePack, devices: &DeviceFile) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (ri, rule) in pack.rules.iter().enumerate() {
        for (si, step) in rule
            .actions
            .iter()
            .chain(rule.policy.fallback.iter())
            .enumerate()
        {
            if let Some(target) = step.target() {
                if !devices.devices.iter().any(|d| d.id == target) {
                    out.push(Diagnostic::error(
                        format!("rules[{ri}].actions[{si}].device"),
                        format!("device `{target}` is not defined in the device file"),
                    ));
                }
            }
        }
    }
    out
}

// --- simulate -----------------------------------------------------------------------------------

fn parse_at(text: &str) -> Result<ScheduleAt> {
    event_spec::schedule_time(&format!("schedule:{text}")).ok_or_else(|| {
        Error::Parse(format!(
            "invalid --at `{text}`: expected HH:MM or HH:MM@day"
        ))
    })
}

fn simulation_clock(at: Option<ScheduleAt>, utc_offset: i32) -> FixedClock {
    let start = match at {
        Some(at) => {
            let day = at.weekday.map_or(0, Weekday::index) as u64;
            let local_ms = day * 86_400_000 + u64::from(at.time.minutes_since_midnight()) * 60_000;
            let offset_ms = i64::from(utc_offset) * 60_000;
            Timestamp::from_millis(
                (MONDAY_EPOCH_MS as i64 + local_ms as i64 - offset_ms).max(0) as u64,
            )
        }
        None => Timestamp::now(),
    };
    FixedClock::new(start)
}

fn parse_health(text: &str) -> Result<(String, DeviceHealth)> {
    let (device, state) = text
        .split_once('=')
        .ok_or_else(|| Error::Parse(format!("invalid --health `{text}`: expected device=state")))?;
    let health = match state {
        "online" => DeviceHealth::Online,
        "degraded" => DeviceHealth::Degraded,
        "offline" => DeviceHealth::Offline,
        "unknown" => DeviceHealth::Unknown,
        _ => {
            return Err(Error::Parse(format!(
                "invalid --health `{text}`: state must be online, degraded, offline or unknown"
            )))
        }
    };
    Ok((device.to_owned(), health))
}

pub fn simulate(args: &SimulateArgs) -> Result<ExitCode> {
    let pack = RulePack::from_path(&args.rules)?;

    let mut registry = DeviceRegistry::new();
    match &args.devices {
        Some(path) => {
            let file = DeviceFile::from_path(path)?;
            let diagnostics = file.validate();
            if has_errors(&diagnostics) {
                return Err(Error::Validation(diagnostics));
            }
            for device in &file.devices {
                registry.register(device.to_device());
            }
        }
        None => {
            // No device file: stand in a virtual device for every target so the pack simulates.
            for rule in &pack.rules {
                for device in rule.targeted_devices() {
                    if !registry.contains(&device) {
                        registry.register(Device::new(device.as_str(), device.as_str()));
                    }
                }
            }
        }
    }
    for text in &args.health {
        let (device, health) = parse_health(text)?;
        if !registry.contains(&device) {
            registry.register(Device::new(device.as_str(), device.as_str()));
        }
        registry.set_health(&device, health)?;
    }

    let mut events: Vec<Event> = Vec::new();
    for spec in &args.events {
        events.extend(event_spec::parse_event(spec, &pack)?);
    }
    let at = match &args.at {
        Some(text) => Some(parse_at(text)?),
        None => args
            .events
            .iter()
            .find_map(|s| event_spec::schedule_time(s)),
    };
    let clock = simulation_clock(at, args.utc_offset);
    let config = EngineConfig {
        mode: Mode::Simulation,
        local_clock: LocalClock::new(args.utc_offset),
        ..EngineConfig::default()
    };
    // The actions dispatcher has no endpoints bound, and simulation never calls it: nothing in this
    // command can send a control message.
    let mut engine = Engine::new(
        pack,
        registry,
        Actions::with_defaults(),
        Arc::new(clock),
        config,
    )?;

    let mut records: Vec<ExecutionRecord> = Vec::new();
    for event in events {
        // A `device:` event carries no previous state of its own; fill it from the registry.
        let event = match event {
            Event::DeviceState {
                device, current, ..
            } => Event::DeviceState {
                previous: engine
                    .registry()
                    .health(&device)
                    .unwrap_or(DeviceHealth::Unknown),
                device,
                current,
            },
            other => other,
        };
        records.extend(engine.handle_event(event));
    }

    match args.format {
        Format::Text => {
            if records.is_empty() {
                println!("SIMULATION — no live actions were sent\n\nNo rule matched the event(s); nothing would have run.");
            }
            for (i, record) in records.iter().enumerate() {
                if i > 0 {
                    println!("\n{}\n", "-".repeat(60));
                }
                print!("{}", render_record(record));
                if record.blocked_by_unknown_condition() {
                    println!("\nhint: a condition could not be evaluated (unknown state); pass e.g. --health <device>=online to supply it");
                }
            }
        }
        Format::Json => println!(
            "{}",
            Value::Array(records.iter().map(machine_result).collect())
        ),
        Format::Trace => println!(
            "{}",
            serde_json::to_string_pretty(&records).map_err(|e| Error::Internal(e.to_string()))?
        ),
    }
    Ok(exit_code(&records))
}

// --- run ----------------------------------------------------------------------------------------

pub fn run(args: &RunArgs) -> Result<ExitCode> {
    init_tracing();
    let mut config = match &args.config {
        Some(path) => ServiceConfig::from_path(path)?,
        None => ServiceConfig::default(),
    };
    if let Some(dir) = &args.state_dir {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
        config.state_db = Some(dir.join("automation.db"));
    }
    if args.simulate {
        config.simulate = true;
    }
    if config.state_db.is_none() {
        eprintln!(
            "warning: no --state-dir given, so nothing is persisted: armed state and one-shot \
             schedules are lost on restart"
        );
    }
    let pack = RulePack::from_path(&args.rules)?;
    let devices = match &args.devices {
        Some(path) => DeviceFile::from_path(path)?,
        None => DeviceFile::default(),
    };
    let store = config.state_db.as_ref().map(Store::open).transpose()?;

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let service = Service::build(pack, devices, config, store, clock)?;
    let handle = service.handle();

    let on_signal = handle.clone();
    ctrlc::set_handler(move || on_signal.shutdown())
        .map_err(|e| Error::Internal(format!("could not install the signal handler: {e}")))?;

    let pid_file = args.state_dir.as_ref().map(|d| d.join("engine.pid"));
    if let Some(path) = &pid_file {
        std::fs::write(path, std::process::id().to_string())
            .map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    }

    let events = handle.subscribe();
    let headless = args.service;
    std::thread::spawn(move || {
        for line in events {
            if headless {
                println!("{line}");
            } else if let Ok(record) = serde_json::from_str::<ExecutionRecord>(&line) {
                println!("{}\n", render_record(&record));
            }
        }
    });

    if !headless {
        eprintln!("running — press Ctrl-C to stop");
    }
    let outcome = service.run();
    if let Some(path) = &pid_file {
        let _ = std::fs::remove_file(path);
    }
    outcome?;
    Ok(ExitCode::Success)
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

// --- watchdog -----------------------------------------------------------------------------------

pub fn watchdog(args: &WatchdogArgs) -> Result<ExitCode> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Internal(format!("cannot locate this executable: {e}")))?;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let on_signal = stop.clone();
    ctrlc::set_handler(move || on_signal.store(true, std::sync::atomic::Ordering::SeqCst))
        .map_err(|e| Error::Internal(format!("could not install the signal handler: {e}")))?;

    let policy = WatchdogPolicy {
        max_restarts: args.max_restarts,
        window: Duration::from_secs(args.window_secs.max(1)),
        ..WatchdogPolicy::default()
    };
    let child_args = args.child.clone();
    let outcome = supervise(
        |_| std::process::Command::new(&exe).args(&child_args).spawn(),
        &policy,
        &stop,
        |event| match event {
            WatchdogEvent::Started { pid, attempt } => {
                eprintln!("watchdog: started engine (pid {pid}, attempt {attempt})")
            }
            WatchdogEvent::Exited { code } => match code {
                Some(code) => eprintln!("watchdog: engine exited with status {code}"),
                None => eprintln!("watchdog: engine was terminated by a signal"),
            },
            WatchdogEvent::SpawnFailed(why) => eprintln!("watchdog: could not start engine: {why}"),
            WatchdogEvent::Restarting { after } => {
                eprintln!("watchdog: restarting in {}ms", after.as_millis())
            }
        },
    );
    match outcome {
        Outcome::CleanExit | Outcome::Stopped => Ok(ExitCode::Success),
        Outcome::GaveUp {
            restarts,
            last_code,
        } => {
            eprintln!(
                "watchdog: engine is crash-looping ({restarts} restarts in {}s, last status {last_code:?}); giving up",
                args.window_secs
            );
            Ok(ExitCode::InternalError)
        }
    }
}

// --- history ------------------------------------------------------------------------------------

pub fn history(args: &HistoryArgs) -> Result<ExitCode> {
    let db = args.state_dir.join("automation.db");
    if !db.exists() {
        return Err(Error::Io(format!(
            "{}: no state database found",
            db.display()
        )));
    }
    let store = Store::open(&db)?;
    let status = args
        .status
        .as_deref()
        .map(|s| {
            serde_json::from_value::<ExecutionStatus>(Value::String(s.to_owned())).map_err(|_| {
                Error::Parse(format!(
                    "unknown status `{s}`; expected success, partial_failure, failed or skipped"
                ))
            })
        })
        .transpose()?;
    let filter = Filter {
        rule: args.rule.clone(),
        status,
        device: args.device.clone(),
        ..Filter::default()
    };
    let records = store.executions(&filter, args.limit.max(1))?;
    match args.format {
        Format::Text => {
            if records.is_empty() {
                println!("no executions recorded");
            }
            for record in &records {
                println!(
                    "{}  {}  {}",
                    record.triggered_at_ms, record.execution_id, record
                );
            }
        }
        Format::Json => println!(
            "{}",
            Value::Array(records.iter().map(machine_result).collect())
        ),
        Format::Trace => println!(
            "{}",
            serde_json::to_string_pretty(&records).map_err(|e| Error::Internal(e.to_string()))?
        ),
    }
    Ok(exit_code(&records))
}
