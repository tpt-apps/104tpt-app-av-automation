"""One-off repair script for model source files (dev tooling, not shipped).

Applies explicit serde renames so the YAML surface matches the dotted catalogue names used in
spec §9 (`control.osc`, `notify.operator`, `workflow.wait`, ...) instead of underscored variant
names.
"""

import pathlib
import re

ROOT = (
    pathlib.Path(__file__).resolve().parents[1]
    / "crates"
    / "tpt-app-av-automation-model"
    / "src"
)


def read(name):
    return (ROOT / name).read_text(encoding="utf-8")


def write(name, text):
    (ROOT / name).write_text(text, encoding="utf-8")


ACTION_RENAMES = {
    "Osc": "control.osc",
    "Midi": "control.midi",
    "DmxChannels": "control.dmx_channels",
    "DmxScene": "control.dmx_scene",
    "DmxUniverse": "control.dmx_universe",
    "MediaVideoSource": "media.video_source",
    "MediaAudioRoute": "media.audio_route",
    "NotifyOperator": "notify.operator",
    "LogIncident": "log.incident",
    "Wait": "workflow.wait",
    "InvokeRule": "workflow.invoke_rule",
    "Exec": "workflow.exec",
    "UseAction": "workflow.use_action",
}

CONDITION_RENAMES = {
    "DeviceHealth": "device_health",
    "TimeWindow": "time_window",
    "DmxChannel": "dmx_channel",
    "DeviceParameter": "device_parameter",
    "RuleArmed": "rule_armed",
    "RuleHasTag": "rule_has_tag",
}

TRIGGER_RENAMES = {
    "Osc": "osc",
    "Midi": "midi",
    "Dmx": "dmx",
    "DeviceState": "device_state",
    "HeartbeatMissed": "heartbeat_missed",
    "DeviceParameter": "device_parameter",
    "Api": "api",
}


def enum_body_span(text, enum_name):
    """Returns (start, end) of the body of `pub enum <enum_name> { ... }`."""
    start = text.index(f"pub enum {enum_name} {{")
    depth = 0
    i = text.index("{", start)
    while True:
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                break
        i += 1
    return start, i


def add_renames(text, renames, enum_name):
    """Inserts `#[serde(rename = "...")]` above each variant of `enum_name`."""
    start, end = enum_body_span(text, enum_name)
    body = text[start:end]

    for variant, wire in renames.items():
        attr = f'#[serde(rename = "{wire}")]\n'
        body = re.sub(
            rf"^(\s*){variant} \{{",
            lambda m: f"{m.group(1)}{attr}{m.group(1)}{variant} {{",
            body,
            flags=re.MULTILINE,
        )
        body = re.sub(
            rf"^(\s*){variant},\n",
            lambda m: f"{m.group(1)}{attr}{m.group(1)}{variant},\n",
            body,
            flags=re.MULTILINE,
        )

    return text[:start] + body + text[end:]


def main():
    write("action.rs", add_renames(read("action.rs"), ACTION_RENAMES, "ActionSpec"))
    write("condition.rs", add_renames(read("condition.rs"), CONDITION_RENAMES, "ConditionSpec"))
    write("trigger.rs", add_renames(read("trigger.rs"), TRIGGER_RENAMES, "TriggerSpec"))
    print("renames applied")


if __name__ == "__main__":
    main()