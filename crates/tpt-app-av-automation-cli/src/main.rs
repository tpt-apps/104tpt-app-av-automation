//! `tpt-av-automation` — validate, simulate and run rule packs (spec §13).
//!
//! The CLI is a thin shell over the same engine the desktop application uses. Its exit codes are a
//! stable contract: 0 success, 1 partial failure, 2 failed, 3 skipped, 4 configuration error,
//! 5 device error, 6 internal error.

mod commands;
mod event_spec;
mod init;

use std::path::PathBuf;
use std::process::ExitCode as ProcessExit;

use clap::{Args, Parser, Subcommand, ValueEnum};
use tpt_app_av_automation_report::ExitCode;

/// Offline-first automation for professional AV, live production and media pipelines.
#[derive(Parser, Debug)]
#[command(
    name = "tpt-av-automation",
    version,
    about,
    long_about = None,
    after_help = "EXAMPLES:
          tpt-av-automation init my-show                       create a working show folder
          tpt-av-automation validate --rules pack.yaml --devices devices.yaml
          tpt-av-automation simulate --rules pack.yaml --devices devices.yaml --event manual
          tpt-av-automation run --service --rules pack.yaml --devices devices.yaml --state-dir ./state

        Rules load disarmed, so nothing is sent until you arm one. Device file format: docs/devices.md."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a ready-to-run show folder (pack, devices, service settings) from a template.
    Init(InitArgs),
    /// List the devices in a device file, check them, and optionally test that each can be opened.
    Devices(DevicesArgs),
    /// Check a rule pack (and optionally its device file and service config) without running it.
    Validate(ValidateArgs),
    /// Evaluate events against a rule pack and show what would happen. Sends nothing, ever.
    Simulate(SimulateArgs),
    /// Run the engine. With --service it runs headless until stopped.
    Run(RunArgs),
    /// Supervise `run` (or any engine command): restart it if it exits unexpectedly.
    Watchdog(WatchdogArgs),
    /// Show recorded executions from the state database.
    History(HistoryArgs),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable text.
    Text,
    /// One machine-readable result per execution.
    Json,
    /// Complete execution records, including every condition and action.
    Trace,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Folder to create the show in.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Template to start from (see `--list`).
    #[arg(long, default_value = "starter")]
    pub template: String,
    /// List the built-in templates and exit.
    #[arg(long)]
    pub list: bool,
    /// Overwrite files that already exist.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct DevicesArgs {
    /// Device file (YAML).
    #[arg(long)]
    pub devices: PathBuf,
    /// Also open each device's endpoint and ping it. UDP devices are connectionless, so for
    /// those this proves the endpoint can be opened, not that the device is listening.
    #[arg(long)]
    pub ping: bool,
    /// Output format (`trace` behaves like `json`).
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,
}

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Rule pack (YAML).
    #[arg(long)]
    pub rules: PathBuf,
    /// Device file; when given, every device a rule targets must be defined in it.
    #[arg(long)]
    pub devices: Option<PathBuf>,
    /// Service configuration file to check as well.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Output format (`trace` behaves like `json`).
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,
}

#[derive(Args, Debug)]
pub struct SimulateArgs {
    /// Rule pack (YAML).
    #[arg(long)]
    pub rules: PathBuf,
    /// Event to inject; repeat to simulate a sequence. See `--help` of the event forms below.
    ///
    /// schedule:HH:MM[@day] | osc:/addr[=v1,v2] | midi:kind:ch:num[:val] | dmx:univ:ch=val |
    /// device:id:state | heartbeat-missed:id:ms | param:id:name=val | manual[:rule] | api:name
    #[arg(long = "event", required = true)]
    pub events: Vec<String>,
    /// Device file, for device registry state.
    #[arg(long)]
    pub devices: Option<PathBuf>,
    /// Initial device health, e.g. `--health projector-1=online`. Repeatable.
    #[arg(long = "health")]
    pub health: Vec<String>,
    /// Local time the simulation runs at, `HH:MM[@day]`. Defaults to the time of a schedule event,
    /// otherwise to now.
    #[arg(long)]
    pub at: Option<String>,
    /// Site UTC offset in minutes.
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    pub utc_offset: i32,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Rule pack (YAML).
    #[arg(long)]
    pub rules: PathBuf,
    /// Device file.
    #[arg(long)]
    pub devices: Option<PathBuf>,
    /// Service configuration file.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Run headless: machine-readable output, suitable for a supervisor or init system.
    #[arg(long)]
    pub service: bool,
    /// Directory for the state database and pid file. Without it nothing is persisted.
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
    /// Force simulation: no rule sends anything, whatever its armed state.
    #[arg(long)]
    pub simulate: bool,
}

#[derive(Args, Debug)]
pub struct WatchdogArgs {
    /// Restarts allowed inside the window before the watchdog gives up.
    #[arg(long, default_value_t = 10)]
    pub max_restarts: u32,
    /// Window, in seconds, over which restarts are counted.
    #[arg(long, default_value_t = 60)]
    pub window_secs: u64,
    /// The command to supervise, e.g. `-- run --rules main.yaml --service`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub child: Vec<String>,
}

#[derive(Args, Debug)]
pub struct HistoryArgs {
    /// State directory used by `run --state-dir`.
    #[arg(long)]
    pub state_dir: PathBuf,
    /// Only this rule id.
    #[arg(long)]
    pub rule: Option<String>,
    /// Only this status: success, partial_failure, failed or skipped.
    #[arg(long)]
    pub status: Option<String>,
    /// Only executions that targeted this device.
    #[arg(long)]
    pub device: Option<String>,
    /// Most executions to show.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,
}

fn main() -> ProcessExit {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() {
                ExitCode::ConfigurationError
            } else {
                ExitCode::Success
            };
            let _ = e.print();
            return ProcessExit::from(code as u8);
        }
    };
    let result = match cli.command {
        Command::Init(args) => init::init(&args),
        Command::Devices(args) => commands::devices(&args),
        Command::Validate(args) => commands::validate(&args),
        Command::Simulate(args) => commands::simulate(&args),
        Command::Run(args) => commands::run(&args),
        Command::Watchdog(args) => commands::watchdog(&args),
        Command::History(args) => commands::history(&args),
    };
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            if let tpt_app_av_automation_core::Error::Validation(diagnostics) = &error {
                for d in diagnostics {
                    eprintln!("  {d}");
                }
            }
            ExitCode::from_error(&error)
        }
    };
    ProcessExit::from(code as u8)
}
