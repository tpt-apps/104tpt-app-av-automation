//! Built-in starter templates: a rule pack plus the devices it uses.
//!
//! The same files ship under `rules/templates/`; they are embedded here so the CLI's `init` and
//! the desktop app's "start from a template" work without any files on disk.

/// One built-in template.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    /// Short name, e.g. `classroom`.
    pub name: &'static str,
    /// One line describing what it is for.
    pub about: &'static str,
    /// The rule pack YAML.
    pub pack: &'static str,
    /// The devices YAML the pack targets.
    pub devices: &'static str,
}

macro_rules! template {
    ($name:literal, $about:literal) => {
        Template {
            name: $name,
            about: $about,
            pack: include_str!(concat!("../../../rules/templates/", $name, ".yaml")),
            devices: include_str!(concat!("../../../rules/templates/", $name, ".devices.yaml")),
        }
    };
}

const TEMPLATES: &[Template] = &[
    template!(
        "starter",
        "two example rules on virtual devices; needs no hardware"
    ),
    template!("classroom", "power a classroom up and down on school days"),
    template!(
        "meeting-room",
        "join/leave cues wake the display and lights"
    ),
    template!("worship", "Sunday service start and wind-down"),
    template!("theatre", "MIDI cue stack recalling lighting presets"),
    template!("museum", "unattended gallery open/close with a media loop"),
    template!(
        "exterior-lights",
        "facade lights following sunset and sunrise"
    ),
    template!("failover", "switch to a backup input when a display drops"),
];

/// Every built-in template.
pub fn all() -> &'static [Template] {
    TEMPLATES
}

/// Looks a template up by name.
pub fn find(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_app_av_automation_devices::DeviceFile;
    use tpt_app_av_automation_model::RulePack;

    #[test]
    fn every_built_in_template_is_valid_and_consistent() {
        for t in all() {
            let pack = RulePack::from_yaml_str(t.pack)
                .unwrap_or_else(|e| panic!("template `{}` pack: {e}", t.name));
            let devices = DeviceFile::from_yaml_str(t.devices)
                .unwrap_or_else(|e| panic!("template `{}` devices: {e}", t.name));
            let problems = devices.validate();
            assert!(problems.is_empty(), "template `{}`: {problems:?}", t.name);
            assert!(
                pack.rules.iter().all(|r| !r.armed),
                "template `{}` must load disarmed",
                t.name
            );
            // Every device a rule targets must exist in the template's own device file.
            let known: Vec<&str> = devices.devices.iter().map(|d| d.id.as_str()).collect();
            for rule in &pack.rules {
                for step in rule.actions.iter().chain(rule.policy.fallback.iter()) {
                    if let Some(device) = step.target() {
                        assert!(
                            known.contains(&device),
                            "template `{}` rule `{}` uses unknown device `{device}`",
                            t.name,
                            rule.id
                        );
                    }
                }
            }
        }
    }
}
