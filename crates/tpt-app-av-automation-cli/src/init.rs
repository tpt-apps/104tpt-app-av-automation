//! `init`: scaffold a working show folder from a built-in template.
//!
//! The templates are the same files shipped under `rules/templates/`, embedded at build time, so a
//! new user can go from nothing to a validated, simulatable show without copying files by hand.

use std::path::Path;

use tpt_app_av_automation_core::{Error, Result};
use tpt_app_av_automation_devices::DeviceFile;
use tpt_app_av_automation_model::RulePack;
use tpt_app_av_automation_report::ExitCode;

use tpt_app_av_automation_service::templates;

use crate::InitArgs;

const SERVICE_YAML: &str = "\
# Service settings for `tpt-av-automation run --service --config service.yaml`.
# Everything here is optional.
tick_ms: 100
# utc_offset_minutes: 0
# listeners:                       # inbound control traffic
#   - { protocol: osc, bind: \"0.0.0.0:9000\" }
# api:                             # local REST/WebSocket API; off unless enabled, loopback only
#   enabled: true
#   token: change-me-to-a-long-random-string
";

pub fn init(args: &InitArgs) -> Result<ExitCode> {
    if args.list {
        for t in templates::all() {
            println!("{:<16} {}", t.name, t.about);
        }
        return Ok(ExitCode::Success);
    }

    let Some(template) = templates::find(&args.template) else {
        eprintln!(
            "available templates: {}",
            templates::all()
                .iter()
                .map(|t| t.name)
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Err(Error::NotFound {
            kind: "template",
            id: args.template.clone(),
        });
    };

    // A template that does not validate is a bug in the template, caught here as well as in tests.
    let pack = RulePack::from_yaml_str(template.pack)?;
    let devices = DeviceFile::from_yaml_str(template.devices)?;
    debug_assert!(devices.validate().is_empty());

    let dir = &args.dir;
    let files = [
        ("pack.yaml", template.pack),
        ("devices.yaml", template.devices),
        ("service.yaml", SERVICE_YAML),
    ];
    if !args.force {
        for (name, _) in &files {
            let path = dir.join(name);
            if path.exists() {
                return Err(Error::Io(format!(
                    "{} already exists; use --force to overwrite",
                    path.display()
                )));
            }
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    for (name, content) in &files {
        write(&dir.join(name), content)?;
    }

    println!(
        "Created {} from the `{}` template ({} rule(s), {} device(s)):",
        dir.display(),
        template.name,
        pack.rules.len(),
        devices.devices.len()
    );
    for (name, _) in &files {
        println!("  {}", dir.join(name).display());
    }
    println!();
    println!("Next steps:");
    println!("  tpt-av-automation validate --rules pack.yaml --devices devices.yaml");
    println!(
        "  tpt-av-automation simulate --rules pack.yaml --devices devices.yaml --event manual"
    );
    println!("  (edit the addresses in devices.yaml to match your equipment, then arm a rule)");
    println!("Every rule starts disarmed, so nothing is sent until you arm it.");
    Ok(ExitCode::Success)
}

fn write(path: &Path, content: &str) -> Result<()> {
    std::fs::write(path, content).map_err(|e| Error::Io(format!("{}: {e}", path.display())))
}
