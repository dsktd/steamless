//! The controls of one target: its kind, then its arguments as the kind takes them.

use dioxus::prelude::*;

use dioxus_primitives::select;

use crate::components::input::Input;
use crate::components::select::SelectOption;
use crate::draft;
use crate::editor::Editor;

/// The chevron's colour: usvg draws the icon and sees no page variable.
const ICON: &str = "#8a8a8a";

/// A plain pick among `(value, label)` options; the value is a String either way. The styled
/// Select, but the trigger shows the current label itself rather than asking the options.
#[component]
pub fn Pick(
    options: Vec<(String, String)>,
    value: ReadSignal<String>,
    onchange: Callback<String>,
    #[props(default)] disabled: bool,
    /// Takes the focus when it mounts.
    #[props(default)]
    focus: bool,
) -> Element {
    let current = use_memo(move || Some(value()));
    let shown = options
        .iter()
        .find(|(option, _)| *option == value())
        .map(|(_, label)| label.clone())
        .unwrap_or_default();
    rsx! {
        select::Select::<String> {
            class: "dx-select",
            value: Some(ReadSignal::from(current)),
            disabled,
            on_value_change: move |picked: Option<String>| {
                if let Some(picked) = picked {
                    onchange.call(picked);
                }
            },
            select::SelectTrigger {
                class: "dx-select-trigger",
                autofocus: focus,
                span { {shown} }
                svg {
                    class: "dx-select-expand-icon",
                    width: "20", height: "20", view_box: "0 0 24 24",
                    fill: "none", stroke: ICON, stroke_width: "2",
                    stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "m6 9 6 6 6-6" }
                }
            }
            select::SelectList {
                class: "dx-select-list",
                for (index, (option, label)) in options.into_iter().enumerate() {
                    SelectOption::<String> { key: "{option}", index, value: option, text_value: label.clone(), {label} }
                }
            }
        }
    }
}

/// Kind select and argument controls for one target; `set(None)` removes it. `unset` labels the
/// empty choice; without it there is none. `skip` names kinds not offered here.
#[component]
pub fn TargetCell(
    editor: Signal<Editor>,
    source: String,
    target: Option<String>,
    set: Callback<Option<String>>,
    unset: Option<String>,
    skip: Vec<String>,
    /// The kind select takes the focus when it mounts.
    #[props(default)]
    focus: bool,
) -> Element {
    let guard = editor.read();
    let vocab = guard.vocab.as_ref().unwrap();
    let kinds = guard.kinds();
    let (kind, args) = match &target {
        Some(target) => kinds.parse(target),
        None => (Some(String::new()), Vec::new()),
    };
    let mut options: Vec<(String, String)> = kinds
        .allowed(vocab, &source, kind.as_deref())
        .into_iter()
        .filter(|name| !skip.contains(name))
        .map(|name| (name.clone(), name))
        .collect();
    if let Some(unset) = &unset {
        options.insert(0, (String::new(), unset.clone()));
    }
    if kind.is_none() {
        options.push((
            "?".to_string(),
            format!("unknown: {}", target.clone().unwrap_or_default()),
        ));
    }
    let chosen = kind.as_deref().and_then(|name| kinds.named(name)).cloned();
    let profiles = draft::profile_names(&guard.draft);
    drop(guard);
    let kinds_ = kinds.clone();
    let picked = move |name: String| {
        if name.is_empty() || name == "?" {
            return set.call(None);
        }
        let Some(chosen) = kinds_.named(&name) else {
            return;
        };
        let first = chosen.args.first().cloned().unwrap_or_default();
        let args = vec![first; chosen.count as usize];
        set.call(Some(draft::build(chosen, &args)));
    };
    rsx! {
        td {
            div { class: "target",
                Pick { options, value: kind.clone().unwrap_or_else(|| "?".to_string()), onchange: picked, focus }
                if let Some(chosen) = chosen {
                    for i in 0..chosen.count as usize {
                        Argument { key: "{i}", chosen: chosen.clone(), args: args.clone(), i, set, profiles: profiles.clone() }
                    }
                }
            }
        }
    }
}

/// The spawn: and workspace: prefixes a desktop argument may take, with their examples.
const TAKES: [(&str, &str); 2] = [("spawn:", "fuzzel"), ("workspace:", "1")];

#[component]
fn Argument(
    chosen: yakin_core::profile::TargetKind,
    args: Vec<String>,
    i: usize,
    set: Callback<Option<String>>,
    profiles: Vec<String>,
) -> Element {
    let arg = args.get(i).cloned().unwrap_or_default();
    let update = {
        let chosen = chosen.clone();
        let args = args.clone();
        move |value: String| {
            let mut next = args.clone();
            next.resize(chosen.count as usize, String::new());
            next[i] = value;
            set.call(Some(draft::build(&chosen, &next)));
        }
    };
    let name = chosen.name.as_str();
    if name == "key" || name == "keys" || name == "flicks" {
        // key takes a combo (KEY_LEFTMETA+KEY_D); a flicks slot any target, a bare name a key
        let placeholder = if name == "key" {
            "KEY_A or KEY_LEFTALT+KEY_TAB"
        } else {
            ""
        };
        return rsx! {
            Input { value: arg, placeholder,
                onchange: move |e: FormEvent| update(e.value().trim().to_string()) }
        };
    }
    if draft::is_action(&chosen) {
        // The named actions, or one taking an argument (spawn:, workspace:) in a text field
        let takes: Vec<(&str, &str)> = if chosen.prefix == "desktop:" {
            TAKES.to_vec()
        } else {
            TAKES[..1].to_vec()
        };
        let taken = takes
            .iter()
            .find(|(prefix, _)| arg.starts_with(prefix))
            .copied();
        let names: Vec<(String, String)> = chosen
            .args
            .iter()
            .map(|a| (a.clone(), a.clone()))
            .chain(takes.iter().map(|(prefix, _)| {
                (
                    prefix.to_string(),
                    format!("{}…", &prefix[..prefix.len() - 1]),
                )
            }))
            .collect();
        let update_pick = update.clone();
        let takes_ = takes.clone();
        return rsx! {
            Pick { options: names, value: taken.map_or(arg.clone(), |(prefix, _)| prefix.to_string()),
                onchange: move |name: String| {
                    match takes_.iter().find(|(prefix, _)| *prefix == name) {
                        Some((prefix, example)) => update_pick(format!("{prefix}{example}")),
                        None => update_pick(name),
                    }
                }
            }
            if let Some((prefix, _)) = taken {
                Input { value: arg[prefix.len()..].to_string(),
                    placeholder: if prefix == "spawn:" { "command" } else { "number" },
                    onchange: move |e: FormEvent| update(format!("{prefix}{}", e.value().trim())) }
            }
        };
    }
    if name == "profile" {
        // next and prev, then every profile of the draft, so a new one is offered before saving
        let mut names: Vec<String> = chosen.args.clone();
        for extra in profiles.iter().chain(std::iter::once(&arg)) {
            if !names.contains(extra) {
                names.push(extra.clone());
            }
        }
        return rsx! {
            Pick { options: names.into_iter().map(|n| (n.clone(), n)).collect::<Vec<_>>(), value: arg, onchange: update }
        };
    }
    rsx! {
        Pick { options: chosen.args.iter().map(|a| (a.clone(), a.clone())).collect::<Vec<_>>(), value: arg, onchange: update }
    }
}
