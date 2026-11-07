//! The editor's model: a draft of the effective config (target strings, not resolved bindings)
//! and what the vocabulary allows. Saved whole with `SetConfig`; the daemon writes what differs
//! from the other layers to its editor drop-in. No UI here, so it is tested on its own.

use std::collections::BTreeMap;

use regex::Regex;
use yakin_core::profile::ActivatorsToml;
use yakin_core::profile::BindingToml;
use yakin_core::profile::ConfigToml;
use yakin_core::profile::ProfileToml;
use yakin_core::profile::TargetKind;
use yakin_core::profile::Vocabulary;
use yakin_ipc::IpcConfig;

/// A button's activator slots besides press.
pub const SLOTS: [&str; 4] = ["long", "double", "start", "release"];
/// A button's timing knobs with their defaults.
pub const TIMINGS: [(&str, &str); 3] = [
    ("long_ms", "400"),
    ("double_ms", "250"),
    ("turbo_ms", "off"),
];
/// `layer:hold:<name>` and `layer:toggle:<name>` are not in the daemon's vocabulary (layers belong
/// to a profile), so the editor adds them as kinds of every button, naming the layers of the
/// profile being edited.
pub const LAYER_KINDS: [&str; 2] = ["hold", "toggle"];

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Group {
    Buttons,
    Axes,
}
impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Group::Buttons => "buttons",
            Group::Axes => "axes",
        }
    }
}

/// The target kinds offered while editing one profile: the vocabulary's and the layer kinds.
#[derive(Clone, Debug, PartialEq)]
pub struct Kinds(Vec<TargetKind>);
impl Kinds {
    pub fn new(
        vocab: &Vocabulary,
        layers: &[String],
    ) -> Self {
        let mut kinds = vocab.kinds.clone();
        for mode in LAYER_KINDS {
            kinds.push(TargetKind {
                name: format!("layer:{mode}"),
                prefix: format!("layer:{mode}:"),
                args: layers.to_vec(),
                count: 1,
            });
        }
        Self(kinds)
    }

    pub fn named(
        &self,
        name: &str,
    ) -> Option<&TargetKind> {
        self.0.iter().find(|kind| kind.name == name)
    }

    /// The kind names `source` accepts: the vocabulary's, and the layer kinds on a button when
    /// there is a layer to name (or `current` already is one).
    pub fn allowed(
        &self,
        vocab: &Vocabulary,
        source: &str,
        current: Option<&str>,
    ) -> Vec<String> {
        let mut names = vocab.allowed.get(source).cloned().unwrap_or_default();
        if vocab.buttons.iter().any(|button| button == source) {
            for mode in LAYER_KINDS {
                let name = format!("layer:{mode}");
                let has_layers = self.named(&name).is_some_and(|kind| !kind.args.is_empty());
                if has_layers || current == Some(name.as_str()) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// The kind of a target string and its arguments, by the kinds' prefixes; no kind for a
    /// target the vocabulary does not know.
    pub fn parse(
        &self,
        target: &str,
    ) -> (Option<String>, Vec<String>) {
        for kind in &self.0 {
            if kind.count == 0 {
                if target == kind.name {
                    return (Some(kind.name.clone()), Vec::new());
                }
                continue;
            }
            let Some(rest) = target.strip_prefix(kind.prefix.as_str()) else {
                continue;
            };
            let args: Vec<String> = rest.split(',').map(str::to_string).collect();
            // pad: is shared by buttons, sticks and triggers; the argument tells them apart.
            // Keys (combos joined by +), profile names and desktop arguments (spawn:,
            // workspace:) are open-ended
            let action = is_action(kind);
            let takes = action
                && (args[0].starts_with("spawn:")
                    || (kind.prefix == "desktop:" && args[0].starts_with("workspace:")));
            let open = kind.name == "key" || kind.name == "profile" || takes;
            if kind.count == 1 && !open && !kind.args.contains(&args[0]) {
                continue;
            }
            // A spawn command may hold commas
            if action {
                return (Some(kind.name.clone()), vec![rest.to_string()]);
            }
            return (Some(kind.name.clone()), args);
        }
        (None, Vec::new())
    }
}

/// desktop: and its raw form niri: name actions, some with an argument after a colon.
pub fn is_action(kind: &TargetKind) -> bool {
    kind.prefix == "desktop:" || kind.prefix == "niri:"
}

pub fn build(
    kind: &TargetKind,
    args: &[String],
) -> String {
    if kind.count == 0 {
        return kind.name.clone();
    }
    format!("{}{}", kind.prefix, args.join(","))
}

/// A button binding as slots: "pad:a" is `{ press: "pad:a" }`.
pub fn slots(binding: Option<&BindingToml>) -> ActivatorsToml {
    match binding {
        None => ActivatorsToml::default(),
        Some(BindingToml::Target(target)) => ActivatorsToml {
            press: Some(target.clone()),
            ..Default::default()
        },
        Some(BindingToml::Activators(activators)) => activators.clone(),
    }
}

/// Slots back to the shortest binding: a string when only press is set, none when nothing is.
pub fn pack(slots: ActivatorsToml) -> Option<BindingToml> {
    let empty = |slot: &Option<String>| slot.as_deref().is_none_or(str::is_empty);
    let only_press = empty(&slots.long)
        && empty(&slots.double)
        && empty(&slots.start)
        && empty(&slots.release)
        && slots.long_ms.is_none()
        && slots.double_ms.is_none()
        && slots.turbo_ms.is_none();
    if only_press {
        return slots
            .press
            .filter(|press| !press.is_empty())
            .map(BindingToml::Target);
    }
    let clean = |slot: Option<String>| slot.filter(|target| !target.is_empty());
    Some(BindingToml::Activators(ActivatorsToml {
        press: clean(slots.press),
        long: clean(slots.long),
        double: clean(slots.double),
        start: clean(slots.start),
        release: clean(slots.release),
        ..slots
    }))
}

pub fn slot<'a>(
    slots: &'a ActivatorsToml,
    name: &str,
) -> Option<&'a str> {
    match name {
        "press" => slots.press.as_deref(),
        "long" => slots.long.as_deref(),
        "double" => slots.double.as_deref(),
        "start" => slots.start.as_deref(),
        "release" => slots.release.as_deref(),
        _ => unreachable!(),
    }
}

pub fn set_slot(
    slots: &mut ActivatorsToml,
    name: &str,
    target: Option<String>,
) {
    match name {
        "press" => slots.press = target,
        "long" => slots.long = target,
        "double" => slots.double = target,
        "start" => slots.start = target,
        "release" => slots.release = target,
        _ => unreachable!(),
    }
}

pub fn timing(
    slots: &ActivatorsToml,
    knob: &str,
) -> Option<u32> {
    match knob {
        "long_ms" => slots.long_ms,
        "double_ms" => slots.double_ms,
        "turbo_ms" => slots.turbo_ms,
        _ => unreachable!(),
    }
}

pub fn set_timing(
    slots: &mut ActivatorsToml,
    knob: &str,
    ms: Option<u32>,
) {
    match knob {
        "long_ms" => slots.long_ms = ms,
        "double_ms" => slots.double_ms = ms,
        "turbo_ms" => slots.turbo_ms = ms,
        _ => unreachable!(),
    }
}

/// The slots besides press that are set.
pub fn extra(slots: &ActivatorsToml) -> usize {
    SLOTS
        .iter()
        .filter(|name| slot(slots, name).is_some())
        .count()
        + TIMINGS
            .iter()
            .filter(|(knob, _)| timing(slots, knob).is_some())
            .count()
}

/// Every profile name of the draft, `pad` included, sorted.
pub fn profile_names(draft: &ConfigToml) -> Vec<String> {
    let mut names: Vec<String> = draft.profile.keys().cloned().collect();
    if !names.iter().any(|name| name == "pad") {
        names.push("pad".to_string());
    }
    names.sort();
    names
}

/// The bindings of `group` in a profile or one of its layers, as `BindingToml` either way.
pub fn binding(
    profile: &ProfileToml,
    layer: Option<&str>,
    group: Group,
    source: &str,
) -> Option<BindingToml> {
    let (buttons, axes) = match layer {
        Some(name) => {
            let layer = profile.layer.get(name)?;
            (&layer.buttons, &layer.axes)
        }
        None => (&profile.buttons, &profile.axes),
    };
    match group {
        Group::Buttons => buttons.get(source).cloned(),
        Group::Axes => axes.get(source).cloned().map(BindingToml::Target),
    }
}

/// Set or remove a binding in the profile, creating the layer when it names one.
pub fn set_binding(
    profile: &mut ProfileToml,
    layer: Option<&str>,
    group: Group,
    source: &str,
    binding: Option<BindingToml>,
) {
    let (buttons, axes) = match layer {
        Some(name) => {
            let layer = profile.layer.entry(name.to_string()).or_default();
            (&mut layer.buttons, &mut layer.axes)
        }
        None => (&mut profile.buttons, &mut profile.axes),
    };
    match (group, binding) {
        (Group::Buttons, Some(binding)) => _ = buttons.insert(source.to_string(), binding),
        (Group::Buttons, None) => _ = buttons.remove(source),
        (Group::Axes, Some(binding)) => {
            let target = binding.press().unwrap_or_default().to_string();
            axes.insert(source.to_string(), target);
        }
        (Group::Axes, None) => _ = axes.remove(source),
    }
}

/// Every layer but the editor's own drop-in, each with its tag: "baseline" or its file name.
pub fn under_layers(file: &IpcConfig) -> Vec<(String, &ConfigToml)> {
    let mut layers = Vec::new();
    if let Some(baseline) = &file.baseline {
        layers.push(("baseline".to_string(), &baseline.toml));
    }
    for layer in &file.dropins {
        if layer.path == file.editor_path {
            continue;
        }
        let tag = layer
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&layer.path)
            .to_string();
        layers.push((tag, &layer.toml));
    }
    layers
}

/// The value a binding has without the editor's drop-in, and the layer that sets it.
pub fn under_binding(
    file: &IpcConfig,
    editing: &str,
    layer: Option<&str>,
    group: Group,
    source: &str,
) -> (Option<BindingToml>, String) {
    let mut found = (None, String::new());
    for (tag, toml) in under_layers(file) {
        let Some(profile) = toml.profile.get(editing) else {
            continue;
        };
        if let Some(value) = binding(profile, layer, group, source) {
            found = (Some(value), tag);
        }
    }
    found
}

/// Only a profile the editor's drop-in alone defines; the others would come back on reload.
pub fn deletable(
    file: &IpcConfig,
    editing: &str,
) -> bool {
    editing != "pad"
        && !under_layers(file)
            .iter()
            .any(|(_, toml)| toml.profile.contains_key(editing))
}

/// Targets of the profile that name `from` as a layer, pointed at `to`. Walks the values, so any
/// name, `$`, quotes and backslashes too, is taken literally.
pub fn retarget(
    profile: &mut ProfileToml,
    from: &str,
    to: &str,
) {
    let swap = |target: &mut String| {
        for mode in LAYER_KINDS {
            if *target == format!("layer:{mode}:{from}") {
                *target = format!("layer:{mode}:{to}");
            }
        }
    };
    let swap_binding = |binding: &mut BindingToml| match binding {
        BindingToml::Target(target) => swap(target),
        BindingToml::Activators(slots) => {
            for name in ["press", "long", "double", "start", "release"] {
                if let Some(mut target) = slot(slots, name).map(str::to_string) {
                    swap(&mut target);
                    set_slot(slots, name, Some(target));
                }
            }
        }
    };
    for binding in profile.buttons.values_mut() {
        swap_binding(binding);
    }
    for layer in profile.layer.values_mut() {
        for binding in layer.buttons.values_mut() {
            swap_binding(binding);
        }
    }
    for chord in &mut profile.chord {
        for binding in chord.buttons.values_mut() {
            swap_binding(binding);
        }
    }
}

/// Clears every binding of the profile, its layers and chords that targets layer `name`, so a
/// deleted layer leaves nothing pointing at it. An activator loses only the slots that target
/// it; a chord member left with nothing becomes "none", as the chord editor does.
pub fn untarget(
    profile: &mut ProfileToml,
    name: &str,
) {
    let gone: Vec<String> = LAYER_KINDS
        .iter()
        .map(|mode| format!("layer:{mode}:{name}"))
        .collect();
    let strip = |binding: &BindingToml| -> Option<BindingToml> {
        match binding {
            BindingToml::Target(target) => (!gone.contains(target)).then(|| binding.clone()),
            BindingToml::Activators(slots) => {
                let mut kept = slots.clone();
                for slot_name in ["press", "long", "double", "start", "release"] {
                    if slot(slots, slot_name)
                        .is_some_and(|target| gone.contains(&target.to_string()))
                    {
                        set_slot(&mut kept, slot_name, None);
                    }
                }
                pack(kept)
            }
        }
    };
    let strip_map = |map: &mut BTreeMap<String, BindingToml>| {
        for (source, binding) in std::mem::take(map) {
            if let Some(kept) = strip(&binding) {
                map.insert(source, kept);
            }
        }
    };
    strip_map(&mut profile.buttons);
    for layer in profile.layer.values_mut() {
        strip_map(&mut layer.buttons);
    }
    for chord in &mut profile.chord {
        for binding in chord.buttons.values_mut() {
            *binding = strip(binding).unwrap_or_else(|| BindingToml::Target("none".to_string()));
        }
    }
}

/// A save error placed where it belongs: "Profile x: buttons.a: why" next to its row (and the
/// profile it names), "menu.items.N: why" and "auto.rule.N.field: why" next to theirs, anything
/// else above the table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    pub profile: Option<String>,
    pub path: Option<String>,
    pub text: String,
}
pub fn place(err: &str) -> Placed {
    let row = Regex::new(r"^Profile (\S+): ((?:buttons|axes|chord\.\d+)\.\w+): (.*)$").unwrap();
    if let Some(found) = row.captures(err) {
        return Placed {
            profile: Some(found[1].to_string()),
            path: Some(found[2].to_string()),
            text: found[3].to_string(),
        };
    }
    let section = Regex::new(r"^((?:menu\.items|auto\.rule)\.\d+)(?:\.\w+)?: (.*)$").unwrap();
    if let Some(found) = section.captures(err) {
        return Placed {
            profile: None,
            path: Some(found[1].to_string()),
            text: err.to_string(),
        };
    }
    Placed {
        profile: None,
        path: None,
        text: err.to_string(),
    }
}

/// Escaped for a regex that matches the app id exactly.
pub fn exact(app_id: &str) -> String {
    format!("^{}$", regex::escape(app_id))
}
