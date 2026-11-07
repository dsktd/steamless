//! Steam Input controller configs (KeyValues `.vdf`) to yakin profiles. Only the subset the
//! daemon executes is converted; everything else becomes a `# skipped:` line and a tally count.

pub mod keys;
pub mod tally;
pub mod vdf;

use std::collections::BTreeMap;
use std::collections::HashMap;

use anyhow::anyhow;
use yakin_core::Result;
use yakin_core::profile::ActivatorsToml;
use yakin_core::profile::BindingToml;
use yakin_core::profile::ChordToml;
use yakin_core::profile::ConfigToml;
use yakin_core::profile::LayerToml;
use yakin_core::profile::ProfileToml;

use crate::tally::Kind;
use crate::tally::Tally;
use crate::vdf::Obj;

/// Steam sensitivity is a percentage around 100; our `mouse.speed` is a factor around 1.
const SENSITIVITY: f32 = 100.0;

/// Pixels a second at full deflection of a `joystick_mouse` at sensitivity 100. Steam's own
/// speed depends on its curve and outer ring, so this is rough: about a 1080p screen's height a
/// second, our default `mouse.stick_speed`.
const STICK_SPEED: f32 = 1000.0;

/// Group settings the import turns into profile knobs, by mode.
const SETTINGS: [(&str, &str); 4] = [
    ("absolute_mouse", "sensitivity"),
    ("gyro_to_mouse", "sensitivity"),
    ("joystick_mouse", "sensitivity"),
    ("trigger", "output_trigger"),
];

/// `xinput_button` names, any case, to `pad:` targets.
const PAD: [(&str, &str); 19] = [
    ("A", "south"),
    ("B", "east"),
    ("X", "west"),
    ("Y", "north"),
    ("SHOULDER_LEFT", "lb"),
    ("SHOULDER_RIGHT", "rb"),
    ("SELECT", "select"),
    ("BACK", "select"),
    ("START", "start"),
    ("GUIDE", "guide"),
    ("JOYSTICK_LEFT", "thumbl"),
    ("JOYSTICK_RIGHT", "thumbr"),
    ("DPAD_UP", "dpad_up"),
    ("DPAD_DOWN", "dpad_down"),
    ("DPAD_LEFT", "dpad_left"),
    ("DPAD_RIGHT", "dpad_right"),
    ("TRIGGER_LEFT", "lt"),
    ("TRIGGER_RIGHT", "rt"),
    ("GUIDE_BUTTON", "guide"),
];

/// Steam activators to our binding slots; Soft_Press is an analog threshold we do not read.
const ACTIVATORS: [(&str, &str); 5] = [
    ("full_press", "press"),
    ("long_press", "long"),
    ("double_press", "double"),
    ("start_press", "start"),
    ("release", "release"),
];

/// Activator settings the import turns into binding knobs.
const ACTIVATOR_SETTINGS: [&str; 4] = [
    "long_press_time",
    "doubetap_max_duration",
    "repeat_rate",
    "hold_repeats",
];

/// Steam's per-activator `haptic_intensity` (off, low, medium, high) to `haptics.click`. The
/// corpus has 0, 1 and 2; 3 is Steam's "high" in its UI.
const HAPTIC_INTENSITY: [f32; 4] = [0.0, 0.2, 0.4, 0.7];

/// Steam input names to our button names. `click` depends on the source, see [`click`].
/// Valve's `button_escape` is the right face button (menu), `button_menu` the left one (view).
const BUTTONS: [(&str, &str); 19] = [
    ("button_a", "a"),
    ("button_b", "b"),
    ("button_x", "x"),
    ("button_y", "y"),
    ("dpad_north", "dpad_up"),
    ("dpad_south", "dpad_down"),
    ("dpad_west", "dpad_left"),
    ("dpad_east", "dpad_right"),
    ("left_bumper", "lb"),
    ("right_bumper", "rb"),
    ("button_escape", "menu"),
    ("button_menu", "view"),
    ("button_back_left", "grip_l2"),
    ("button_back_left_upper", "grip_l1"),
    ("button_back_right", "grip_r2"),
    ("button_back_right_upper", "grip_r1"),
    ("button_capture", "quick"),
    ("left_stick_click", "lstick_click"),
    ("right_stick_click", "rstick_click"),
];

/// Steam sources to our axes.
const AXES: [(&str, &str); 7] = [
    ("gyro", "gyro"),
    ("joystick", "lstick"),
    ("right_joystick", "rstick"),
    ("left_trackpad", "lpad"),
    ("right_trackpad", "rpad"),
    ("left_trigger", "lt"),
    ("right_trigger", "rt"),
];

/// What a source's `click` input is.
const CLICKS: [(&str, &str); 6] = [
    ("joystick", "lstick_click"),
    ("right_joystick", "rstick_click"),
    ("left_trackpad", "lpad_click"),
    ("right_trackpad", "rpad_click"),
    ("left_trigger", "lt_full"),
    ("right_trigger", "rt_full"),
];

fn find(
    table: &[(&str, &'static str)],
    name: &str,
) -> Option<&'static str> {
    table
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, ours)| *ours)
}

/// One converted file: a profile per action set, a `# skipped:` line per thing it left out, and
/// the counts.
#[derive(Debug, Default)]
pub struct Imported {
    /// By name: the config's name alone for one action set, else `<name>-<set>`.
    pub profiles: BTreeMap<String, ProfileToml>,
    pub skipped: Vec<String>,
    pub tally: Tally,
}
impl Imported {
    /// The profiles as a config file, the skipped lines on top.
    pub fn toml(&self) -> Result<String> {
        let config = ConfigToml {
            profile: self.profiles.clone(),
            ..ConfigToml::default()
        };
        let mut out: String = self
            .skipped
            .iter()
            .map(|line| format!("# skipped: {line}\n"))
            .collect();
        out.push_str(&toml::to_string(&config)?);
        Ok(out)
    }
}

/// One input's binding strings under one activator; the old flat `bindings` form is a Full_Press.
struct Input<'a> {
    name: &'a str,
    activator: &'a str,
    bindings: Vec<&'a str>,
    settings: Vec<(&'a str, &'a str)>,
}
impl Input<'_> {
    /// Our binding slot for the activator, or why it has none.
    fn slot(&self) -> std::result::Result<&'static str, &'static str> {
        if self.activator.eq_ignore_ascii_case("soft_press") {
            return Err("analog threshold");
        }
        find(&ACTIVATORS, self.activator).ok_or("activator")
    }

    fn setting(
        &self,
        name: &str,
    ) -> Option<&str> {
        self.settings
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    }
}

/// Put `target` in `slot` of `button`'s activators, with the knobs of `input`'s settings.
fn fill(
    activators: &mut ActivatorsToml,
    slot: &str,
    input: &Input,
    target: String,
) {
    let ms = |name: &str| {
        input
            .setting(name)
            .and_then(|value| value.parse::<u32>().ok())
    };
    match slot {
        "press" => activators.press = Some(target),
        "long" => activators.long = Some(target),
        "double" => activators.double = Some(target),
        "start" => activators.start = Some(target),
        _ => activators.release = Some(target),
    }
    if let Some(ms) = ms("long_press_time").filter(|ms| *ms > 0) {
        activators.long_ms = Some(ms);
    }
    if let Some(ms) = ms("doubetap_max_duration").filter(|ms| *ms > 0) {
        activators.double_ms = Some(ms);
    }
    let repeats = input
        .setting("hold_repeats")
        .is_some_and(|value| value != "0");
    if slot == "press"
        && repeats
        && let Some(rate) = ms("repeat_rate").filter(|rate| *rate > 0)
    {
        activators.turbo_ms = Some((1000 / rate).max(1));
    }
}

/// A plain target string when only `press` is set, else the table.
fn pack(activators: ActivatorsToml) -> BindingToml {
    let only_press = ActivatorsToml {
        press: activators.press.clone(),
        ..ActivatorsToml::default()
    };
    match activators.press.clone() {
        Some(press) if activators == only_press => BindingToml::Target(press),
        _ => BindingToml::Activators(activators),
    }
}

fn inputs<'a>(group: &'a Obj<'a>) -> Vec<Input<'a>> {
    let mut inputs = Vec::new();
    if let Some(bindings) = vdf::obj(group, "bindings") {
        for (name, binding) in vdf::pairs(bindings) {
            inputs.push(Input {
                name,
                activator: "Full_Press",
                bindings: vec![binding],
                settings: Vec::new(),
            });
        }
    }
    let Some(named) = vdf::obj(group, "inputs") else {
        return inputs;
    };
    for (name, values) in named.iter() {
        for activators in values
            .iter()
            .filter_map(|value| value.get_obj())
            .filter_map(|input| vdf::obj(input, "activators"))
        {
            for (activator, values) in activators.iter() {
                for activator_obj in values.iter().filter_map(|value| value.get_obj()) {
                    let bindings = vdf::obj(activator_obj, "bindings")
                        .map(|bindings| {
                            vdf::all(bindings, "binding")
                                .filter_map(|value| value.get_str())
                                .collect()
                        })
                        .unwrap_or_default();
                    let settings = vdf::obj(activator_obj, "settings")
                        .map(|settings| vdf::pairs(settings).collect())
                        .unwrap_or_default();
                    inputs.push(Input {
                        name,
                        activator,
                        bindings,
                        settings,
                    });
                }
            }
        }
    }
    inputs
}

/// A binding's target: `Ok` with our target string, `Err` with the verb we cannot execute.
fn target(binding: &str) -> std::result::Result<String, String> {
    let binding = binding.split(',').next().unwrap_or_default().trim();
    let (verb, arg) = binding.split_once(' ').unwrap_or((binding, ""));
    let arg = arg.trim();
    let target = match verb {
        "key_press" => keys::key(arg).map(|key| format!("key:{key}")),
        "xinput_button" => find(&PAD, arg).map(|pad| format!("pad:{pad}")),
        "mouse_button" => match arg.to_ascii_uppercase().as_str() {
            "LEFT" | "RIGHT" | "MIDDLE" => Some(format!("mouse:{}", arg.to_ascii_lowercase())),
            _ => None,
        },
        "empty_binding" => Some("none".to_string()),
        _ => None,
    };
    target.ok_or(verb.to_string())
}

/// Several `key_press` bindings on one activator, pressed together: `key:KEY_LEFTCTRL+KEY_C`.
fn combo(bindings: &[&str]) -> Option<String> {
    if bindings.len() < 2 {
        return None;
    }
    let keys: Vec<String> = bindings
        .iter()
        .map(|binding| Some(target(binding).ok()?.strip_prefix("key:")?.to_string()))
        .collect::<Option<_>>()?;
    Some(format!("key:{}", keys.join("+")))
}

fn verb(binding: &str) -> &str {
    binding.split([' ', ',']).next().unwrap_or_default()
}

/// Whether we run a group of `mode` on `source`, and if not, why.
fn supported(
    source: &str,
    mode: &str,
) -> std::result::Result<(), &'static str> {
    let stick = matches!(source, "joystick" | "right_joystick");
    let pad = matches!(source, "left_trackpad" | "right_trackpad");
    let trigger = matches!(source, "left_trigger" | "right_trigger");
    let gyro = source == "gyro";
    match mode {
        "gyro_to_mouse" | "absolute_mouse" if gyro => Ok(()),
        "four_buttons" | "switches" => Ok(()),
        "dpad" if source == "dpad" || stick => Ok(()),
        "joystick_move" | "joystick_mouse" | "flickstick" if stick => Ok(()),
        "trigger" if trigger => Ok(()),
        "absolute_mouse" | "scrollwheel" if pad => Ok(()),
        "dpad" | "joystick_move" | "joystick_mouse" | "flickstick" | "trigger"
        | "absolute_mouse" | "scrollwheel" | "gyro_to_mouse" => Err("mode on this source"),
        _ => Err("mode"),
    }
}

/// A group's mode; the old flat `switch_bindings` of a preset have none.
fn mode<'a>(
    source: &str,
    group: &'a Obj<'a>,
) -> &'a str {
    match vdf::text(group, "mode") {
        Some(mode) => mode,
        None if source == "switch" => "switches",
        None => "?",
    }
}

struct Importer<'a> {
    groups: HashMap<&'a str, &'a Obj<'a>>,
    /// Each preset's profile or layer name, in file order, and whether it is a layer.
    presets: Vec<(String, bool)>,
    /// The preset being converted.
    profile: ProfileToml,
    /// Shifted groups of the first preset; each `mode_shift` that converts removes its own.
    shifted: Vec<(&'a str, &'a str)>,
    /// The strongest `haptic_intensity` of an executed activator, as `haptics.click`.
    click: Option<f32>,
    out: Imported,
}
impl<'a> Importer<'a> {
    fn skip(
        &mut self,
        kind: Kind,
        name: &str,
        line: String,
    ) {
        self.out.tally.skipped(kind, name);
        self.out.skipped.push(line);
    }

    /// Count everything in a group we do not run, with one line for the whole group.
    fn skip_group(
        &mut self,
        source: &str,
        group: &'a Obj<'a>,
        why: &str,
    ) {
        let mode = mode(source, group);
        self.skip(Kind::Mode, mode, format!("{source} {mode} ({why})"));
        self.out.tally.skipped(Kind::Reason, why);
        for input in inputs(group) {
            self.out.tally.skipped(Kind::Activator, input.activator);
            for binding in input.bindings {
                self.out.tally.skipped(Kind::Verb, verb(binding));
            }
            for (setting, _) in input.settings {
                self.out.tally.skipped(Kind::Setting, setting);
            }
        }
        for (setting, _) in vdf::obj(group, "settings").into_iter().flat_map(vdf::pairs) {
            self.out.tally.skipped(Kind::Setting, setting);
        }
    }

    fn group(
        &mut self,
        source: &str,
        group: &'a Obj<'a>,
    ) {
        let mode = mode(source, group);
        if let Err(why) = supported(source, mode) {
            return self.skip_group(source, group, why);
        }
        self.out.tally.executed(Kind::Mode, mode);
        let axis = find(&AXES, source);
        if let Some(axis) = axis {
            let target = match mode {
                "joystick_move" => Some(if axis == "lstick" {
                    "pad:left"
                } else {
                    "pad:right"
                }),
                "absolute_mouse" | "joystick_mouse" | "gyro_to_mouse" => Some("cursor"),
                "flickstick" => Some("flick"),
                "scrollwheel" => Some("scroll"),
                _ => None,
            };
            if let Some(target) = target {
                self.profile
                    .axes
                    .insert(axis.to_string(), target.to_string());
            }
        }
        for (setting, value) in vdf::obj(group, "settings").into_iter().flat_map(vdf::pairs) {
            if !SETTINGS.contains(&(mode, setting)) {
                self.out.tally.skipped(Kind::Setting, setting);
                continue;
            }
            self.out.tally.executed(Kind::Setting, setting);
            match setting {
                "sensitivity" => {
                    let speed = value.parse::<f32>().unwrap_or(SENSITIVITY) / SENSITIVITY;
                    match (mode, source) {
                        ("joystick_mouse", _) => {
                            self.profile.mouse.stick_speed = Some(speed * STICK_SPEED)
                        }
                        (_, "gyro") => self.profile.gyro.speed = Some(speed),
                        _ => self.profile.mouse.speed = Some(speed),
                    }
                }
                _ => {
                    if value != "0"
                        && let Some(axis) = axis
                    {
                        self.profile
                            .axes
                            .insert(axis.to_string(), format!("pad:{axis}"));
                    }
                }
            }
        }
        let mut stick_keys = BTreeMap::new();
        let mut buttons: BTreeMap<&str, ActivatorsToml> = BTreeMap::new();
        for input in inputs(group) {
            let first = input.bindings.first().copied().unwrap_or_default();
            // Only button presses take activators other than Full_Press
            let plain = !(verb(first) == "mode_shift"
                || (mode == "dpad" && source != "dpad")
                || (mode == "scrollwheel" && input.name.starts_with("scroll_")));
            let slot = match input.slot() {
                Ok("press") => Ok("press"),
                Ok(_) if !plain => Err("activator"),
                slot => slot,
            };
            let Ok(slot) = slot else {
                self.skip_activator(source, mode, &input, slot.unwrap_err());
                continue;
            };
            self.out.tally.executed(Kind::Activator, input.activator);
            self.settings(&input, slot);
            let mut bindings = input.bindings.iter();
            let Some(first) = bindings.next() else {
                continue;
            };
            let combo = combo(&input.bindings);
            for extra in bindings.filter(|_| combo.is_none()) {
                self.skip(
                    Kind::Verb,
                    verb(extra),
                    format!("{source} {mode} {}: {extra} (extra binding)", input.name),
                );
            }
            if mode == "scrollwheel" && input.name.starts_with("scroll_") {
                self.out.tally.executed(Kind::Verb, verb(first));
                continue;
            }
            let button = if input.name.eq_ignore_ascii_case("click") {
                find(&CLICKS, source)
            } else if mode == "dpad" && source != "dpad" {
                None
            } else {
                find(&BUTTONS, input.name)
            };
            if verb(first) == "mode_shift" {
                self.mode_shift(source, mode, button, first);
                continue;
            }
            match combo.map_or_else(|| self.target(first), Ok) {
                Ok(target) if mode == "dpad" && source != "dpad" => {
                    self.out.tally.executed(Kind::Verb, verb(first));
                    stick_keys.insert(input.name.to_ascii_lowercase(), target);
                }
                Ok(target) => match button {
                    Some(button) => {
                        self.out.tally.executed(Kind::Verb, verb(first));
                        fill(buttons.entry(button).or_default(), slot, &input, target);
                    }
                    None => self.skip(
                        Kind::Verb,
                        verb(first),
                        format!("{source} {mode} {}: {first} (input)", input.name),
                    ),
                },
                Err(verb) => self.skip(
                    Kind::Verb,
                    &verb,
                    format!("{source} {mode} {}: {first}", input.name),
                ),
            }
        }
        for (button, activators) in buttons {
            self.profile
                .buttons
                .insert(button.to_string(), pack(activators));
        }
        self.stick_keys(source, stick_keys);
    }

    /// An activator we do not run, with the verbs and settings under it.
    fn skip_activator(
        &mut self,
        source: &str,
        mode: &str,
        input: &Input,
        why: &str,
    ) {
        self.out.tally.skipped(Kind::Activator, input.activator);
        for binding in &input.bindings {
            self.out.tally.skipped(Kind::Verb, verb(binding));
        }
        for (setting, _) in &input.settings {
            self.out.tally.skipped(Kind::Setting, setting);
        }
        let line = format!(
            "{source} {mode} {} {}: {} ({why})",
            input.name,
            input.activator,
            input.bindings.join("; ")
        );
        self.out.skipped.push(line);
    }

    /// Tally an executed activator's settings: its timing ones are ours.
    fn settings(
        &mut self,
        input: &Input,
        slot: &str,
    ) {
        for (setting, value) in &input.settings {
            let lower = setting.to_ascii_lowercase();
            if lower == "haptic_intensity" {
                match value
                    .parse::<usize>()
                    .ok()
                    .and_then(|level| HAPTIC_INTENSITY.get(level))
                {
                    Some(click) => {
                        self.click = Some(self.click.map_or(*click, |known| known.max(*click)));
                        self.out.tally.executed(Kind::Setting, setting);
                    }
                    None => self.out.tally.skipped(Kind::Setting, setting),
                }
                continue;
            }
            let turbo = matches!(lower.as_str(), "repeat_rate" | "hold_repeats");
            if ACTIVATOR_SETTINGS.contains(&lower.as_str()) && (!turbo || slot == "press") {
                self.out.tally.executed(Kind::Setting, setting);
            } else {
                self.out.tally.skipped(Kind::Setting, setting);
            }
        }
    }

    /// A stick in dpad mode is our `keys:` target when all four directions are keys.
    fn stick_keys(
        &mut self,
        source: &str,
        keys: BTreeMap<String, String>,
    ) {
        if keys.is_empty() {
            return;
        }
        let targets: Option<Vec<&str>> = ["dpad_north", "dpad_south", "dpad_west", "dpad_east"]
            .iter()
            .map(|direction| keys.get(*direction).map(String::as_str))
            .collect();
        let axis = find(&AXES, source).unwrap_or(source);
        match targets {
            Some(targets) => {
                // Plain keys stay `keys:`; a combo, mouse or pad button needs `flicks:`
                let names: Option<Vec<&str>> = targets
                    .iter()
                    .map(|target| target.strip_prefix("key:").filter(|key| !key.contains('+')))
                    .collect();
                let target = match names {
                    Some(names) => format!("keys:{}", names.join(",")),
                    None => format!("flicks:{}", targets.join(",")),
                };
                self.profile.axes.insert(axis.to_string(), target);
            }
            None => self
                .out
                .skipped
                .push(format!("{source} dpad: {keys:?} (needs four directions)")),
        }
    }

    /// [`target`], with the action set and layer actions this file's presets resolve.
    fn target(
        &self,
        binding: &str,
    ) -> std::result::Result<String, String> {
        match verb(binding) {
            "controller_action" => self.action(binding).ok_or("controller_action".to_string()),
            _ => target(binding),
        }
    }

    /// `controller_action CHANGE_PRESET <n>` and the layer actions: `n` counts presets from 1.
    fn action(
        &self,
        binding: &str,
    ) -> Option<String> {
        let binding = binding.split(',').next().unwrap_or_default();
        let mut words = binding.split_whitespace().skip(1);
        let action = words.next()?.to_ascii_lowercase();
        let index = words.next()?.parse::<usize>().ok()?.checked_sub(1)?;
        let (name, layer) = self.presets.get(index)?;
        match (action.as_str(), layer) {
            ("change_preset", false) => Some(format!("profile:{name}")),
            ("hold_layer", true) => Some(format!("layer:hold:{name}")),
            ("add_layer", true) => Some(format!("layer:toggle:{name}")),
            _ => None,
        }
    }

    /// `mode_shift <source> <group>` held on `hold`: a chord when the shifted group is buttons.
    fn mode_shift(
        &mut self,
        source: &str,
        mode: &str,
        hold: Option<&str>,
        binding: &str,
    ) {
        let mut args = binding.split_whitespace().skip(1);
        let (Some(shifted), Some(id)) = (args.next(), args.next()) else {
            return self.skip(
                Kind::Verb,
                "mode_shift",
                format!("{source} {mode}: {binding} (malformed)"),
            );
        };
        let Some(group) = self.groups.get(id).copied() else {
            return self.skip(
                Kind::Verb,
                "mode_shift",
                format!("{source} {mode}: {binding} (no group {id})"),
            );
        };
        let shifted_mode = vdf::text(group, "mode").unwrap_or("?");
        let simple = matches!(shifted_mode, "four_buttons" | "switches")
            || (shifted_mode == "dpad" && shifted == "dpad");
        let Some(hold) = hold.filter(|_| simple) else {
            return self.skip(
                Kind::Verb,
                "mode_shift",
                format!("{source} {mode}: {binding} (shifts {shifted} {shifted_mode})"),
            );
        };
        self.shifted.retain(|(_, known)| *known != id);
        self.out.tally.executed(Kind::Verb, "mode_shift");
        self.out.tally.executed(Kind::Mode, shifted_mode);
        let mut buttons: BTreeMap<&str, ActivatorsToml> = BTreeMap::new();
        for input in inputs(group) {
            let button = find(&BUTTONS, input.name);
            let first = input.bindings.first().copied().unwrap_or_default();
            match (input.slot(), button, self.target(first)) {
                (Ok(slot), Some(button), Ok(target)) => {
                    self.out.tally.executed(Kind::Activator, input.activator);
                    self.out.tally.executed(Kind::Verb, verb(first));
                    self.settings(&input, slot);
                    fill(buttons.entry(button).or_default(), slot, &input, target);
                }
                _ => {
                    self.out.tally.skipped(Kind::Activator, input.activator);
                    self.skip(
                        Kind::Verb,
                        verb(first),
                        format!(
                            "{shifted} {shifted_mode} {}: {first} (in chord)",
                            input.name
                        ),
                    );
                }
            }
        }
        let buttons = buttons
            .into_iter()
            .map(|(button, activators)| (button.to_string(), pack(activators)))
            .collect();
        self.profile.chord.push(ChordToml {
            hold: hold.to_string(),
            buttons,
        });
    }

    fn preset(
        &mut self,
        preset: &'a Obj<'a>,
    ) {
        let sources: Vec<(&str, &str, Vec<&str>)> = vdf::obj(preset, "group_source_bindings")
            .into_iter()
            .flat_map(vdf::pairs)
            .map(|(id, binding)| {
                let mut words = binding.split_whitespace();
                let source = words.next().unwrap_or("?");
                (id, source, words.collect())
            })
            .collect();
        // Shifted groups first, so a mode_shift seen before its group still claims it
        for (id, source, state) in &sources {
            if state.contains(&"modeshift") && self.groups.contains_key(id) {
                self.shifted.push((source, id));
            }
        }
        for (id, source, state) in sources {
            let Some(group) = self.groups.get(id).copied() else {
                self.out
                    .skipped
                    .push(format!("{source} group {id} (missing)"));
                continue;
            };
            if state.contains(&"modeshift") {
                continue;
            }
            match state.first() == Some(&"active") {
                true => self.group(source, group),
                false => self.skip_group(source, group, "inactive"),
            }
        }
        if let Some(switches) = vdf::obj(preset, "switch_bindings") {
            self.group("switch", switches);
        }
        for (setting, _) in vdf::obj(preset, "settings")
            .into_iter()
            .flat_map(vdf::pairs)
        {
            self.out.tally.skipped(Kind::Setting, setting);
        }
    }
}

/// A Steam set or layer name as a profile or layer name: lower case, `-` for anything odd.
fn slug(name: &str) -> String {
    name.chars()
        .map(|char| match char.is_ascii_alphanumeric() || char == '_' {
            true => char.to_ascii_lowercase(),
            false => '-',
        })
        .collect()
}

/// Convert one `.vdf` file into profiles named after `name`: each preset is an action set, a
/// profile, unless `action_layers` names it, then it is a layer of its parent set's profile.
pub fn import(
    text: &str,
    name: &str,
) -> Result<Imported> {
    let text = text.trim_start_matches('\u{feff}');
    let vdf = keyvalues_parser::parse(text).map_err(|err| anyhow!("{err}"))?;
    let Some(root) = vdf.value.get_obj() else {
        return Err(anyhow!("{} is not an object", vdf.key));
    };
    if !vdf.key.eq_ignore_ascii_case("controller_mappings") {
        return Err(anyhow!("Expected controller_mappings, got {}", vdf.key));
    }
    let groups = vdf::all(root, "group")
        .filter_map(|group| group.get_obj())
        .filter_map(|group| Some((vdf::text(group, "id")?, group)))
        .collect();
    // Layer name to its parent set's name
    let parents: HashMap<&str, &str> = vdf::obj(root, "action_layers")
        .into_iter()
        .flat_map(|layers| layers.iter())
        .filter_map(|(layer, values)| {
            let parent = values
                .iter()
                .filter_map(|value| value.get_obj())
                .find_map(|layer| vdf::text(layer, "parent_set_name"))?;
            Some((layer.as_ref(), parent))
        })
        .collect();
    let presets: Vec<(&str, &Obj)> = vdf::all(root, "preset")
        .filter_map(|preset| preset.get_obj())
        .map(|preset| (vdf::text(preset, "name").unwrap_or("?"), preset))
        .collect();
    let sets = presets
        .iter()
        .filter(|(set, _)| !parents.contains_key(set))
        .count();
    let profile = |set: &str| match sets {
        1 => name.to_string(),
        _ => format!("{name}-{}", slug(set)),
    };
    let mut importer = Importer {
        groups,
        presets: presets
            .iter()
            .map(|(set, _)| match parents.contains_key(set) {
                true => (slug(set), true),
                false => (profile(set), false),
            })
            .collect(),
        profile: ProfileToml::default(),
        shifted: Vec::new(),
        click: None,
        out: Imported::default(),
    };
    let mut layers = Vec::new();
    for (set, preset) in presets {
        importer.preset(preset);
        for (source, id) in std::mem::take(&mut importer.shifted) {
            let group = importer.groups[id];
            importer.skip_group(source, group, "mode_shift target");
        }
        let mut converted = std::mem::take(&mut importer.profile);
        converted.haptics.click = importer.click.take();
        match parents.get(set) {
            Some(parent) => layers.push((profile(parent), slug(set), converted)),
            None => {
                importer.out.profiles.insert(profile(set), converted);
            }
        }
    }
    for (parent, layer, converted) in layers {
        let Some(profile) = importer.out.profiles.get_mut(&parent) else {
            importer
                .out
                .skipped
                .push(format!("layer {layer} (no set {parent})"));
            continue;
        };
        let buttons = converted.buttons.clone();
        let axes = converted.axes.clone();
        let rest = ProfileToml {
            buttons: BTreeMap::new(),
            axes: BTreeMap::new(),
            ..converted
        };
        if rest != ProfileToml::default() {
            importer.out.skipped.push(format!(
                "layer {layer}: chords and settings (layers have none)"
            ));
        }
        profile.layer.insert(layer, LayerToml { buttons, axes });
    }
    Ok(importer.out)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_combo() {
        let combo = combo(&["key_press LEFT_CONTROL", "key_press C, Copy"]);
        assert_eq!(combo.as_deref(), Some("key:KEY_LEFTCTRL+KEY_C"));
        assert_eq!(super::combo(&["key_press C"]), None);
        assert_eq!(super::combo(&["key_press C", "mouse_button LEFT"]), None);
        assert_eq!(super::combo(&["key_press C", "key_press NOPE"]), None);
    }
}
