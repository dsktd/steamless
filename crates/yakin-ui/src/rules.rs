//! The Focus rules section: `[auto]`, the profile following the focused window.

use dioxus::prelude::*;
use yakin_core::auto::AutoToml;
use yakin_core::auto::RuleToml;

use crate::components::button::Button;
use crate::components::button::ButtonSize;
use crate::components::button::ButtonVariant;
use crate::components::input::Input;
use crate::components::switch::Switch;
use crate::draft;
use crate::editor::Editor;
use crate::live::Live;
use crate::menu::Moves;
use crate::menu::SectionHead;
use crate::target::Pick;

/// Change `[auto]`; one with nothing in it is no `[auto]` at all.
fn edit(
    editor: &mut Editor,
    change: impl FnOnce(&mut AutoToml),
) {
    change(&mut editor.draft.auto);
    let auto = &editor.draft.auto;
    if !auto.enabled && auto.default.is_none() && auto.rule.is_empty() {
        editor.draft.auto = AutoToml::default();
    }
}

/// `[auto]` as the layers under the editor's set it, merged key by key as tables are.
fn under(editor: &Editor) -> (AutoToml, String, bool) {
    let mut merged = AutoToml::default();
    let mut tag = "baseline".to_string();
    let mut set = false;
    for (layer_tag, toml) in draft::under_layers(editor.file.as_ref().unwrap()) {
        if toml.auto == AutoToml::default() {
            continue;
        }
        set = true;
        tag = layer_tag;
        merged.enabled = toml.auto.enabled;
        if toml.auto.default.is_some() {
            merged.default = toml.auto.default.clone();
        }
        if !toml.auto.rule.is_empty() {
            merged.rule = toml.auto.rule.clone();
        }
    }
    (merged, tag, set)
}

#[component]
pub fn RuleSection(
    editor: Signal<Editor>,
    live: Signal<Live>,
) -> Element {
    let guard = editor.read();
    let auto = guard.draft.auto.clone();
    let (under, tag, set) = under(&guard);
    let mine = auto
        != if set {
            under.clone()
        } else {
            AutoToml::default()
        };
    let profiles: Vec<(String, String)> = draft::profile_names(&guard.draft)
        .into_iter()
        .map(|n| (n.clone(), n))
        .collect();
    let fallback: Vec<(String, String)> =
        std::iter::once(("".to_string(), "leave as is".to_string()))
            .chain(profiles.iter().cloned())
            .collect();
    let editing = guard.editing();
    let errors = guard.row_errors.clone();
    drop(guard);
    let status = live.read().auto.clone();
    let focused = status
        .as_ref()
        .map(|auto| auto.focused_app_id.clone())
        .unwrap_or_default();
    let hit = status.as_ref().and_then(|auto| auto.rule);
    let editing_ = editing.clone();
    let focused_ = focused.clone();
    let add = Callback::new(move |app_id: String| {
        let profile = editing_.clone();
        edit(&mut editor.write(), |auto| {
            auto.rule.push(RuleToml {
                app_id,
                title: None,
                profile,
            })
        });
    });
    let swap = Callback::new(move |(a, b): (usize, usize)| {
        edit(&mut editor.write(), |auto| auto.rule.swap(a, b))
    });
    let remove = Callback::new(move |index: usize| {
        edit(&mut editor.write(), |auto| _ = auto.rule.remove(index))
    });
    rsx! {
        SectionHead { title: "Focus rules", mine, tag,
            reset: move |()| editor.write().draft.auto = if set { under.clone() } else { AutoToml::default() } }
        p { class: "inline",
            label { class: "inline",
                Switch { checked: Some(auto.enabled),
                    on_checked_change: move |on: bool| edit(&mut editor.write(), |auto| auto.enabled = on) }
                " Follow the focused window"
            }
            " Default "
            Pick { options: fallback, value: auto.default.clone().unwrap_or_default(),
                onchange: move |name: String| edit(&mut editor.write(), |auto| auto.default = (!name.is_empty()).then_some(name)) }
        }
        p { class: "note",
            if status.is_some() {
                {format!("Focused app id: {}", if focused.is_empty() { "none" } else { focused.as_str() })}
            } else {
                "Focused app id shows while following is on and saved."
            }
        }
        table { class: "auto-rules",
            for (index, rule) in auto.rule.iter().enumerate() {
                tr { key: "{index}", class: if hit == Some(index) { "pressed" },
                    td {
                        Input { value: rule.app_id.clone(), placeholder: "app id regex",
                            onchange: move |e: FormEvent| edit(&mut editor.write(), |auto| auto.rule[index].app_id = e.value()) }
                    }
                    td {
                        Input { value: rule.title.clone().unwrap_or_default(), placeholder: "title regex (optional)",
                            onchange: move |e: FormEvent| edit(&mut editor.write(), |auto| {
                                let title = e.value();
                                auto.rule[index].title = (!title.is_empty()).then_some(title);
                            }) }
                    }
                    td {
                        Pick { options: profiles.clone(), value: rule.profile.clone(),
                            onchange: move |name: String| edit(&mut editor.write(), |auto| auto.rule[index].profile = name) }
                    }
                    Moves { index, len: auto.rule.len(), swap, remove }
                    td { span { class: "row-error", {errors.get(&format!("auto.rule.{index}")).cloned().unwrap_or_default()} } }
                }
            }
        }
        p { class: "inline",
            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, onclick: move |_| add.call(String::new()), "Add rule" }
            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, disabled: focused_.is_empty(),
                onclick: move |_| add.call(draft::exact(&focused_)), "Add rule for this app" }
            span { class: "note", " Rules are tried in order, the first match wins; new rules use the profile being edited." }
        }
    }
}
