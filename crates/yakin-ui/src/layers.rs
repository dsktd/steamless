//! The Layers section: a layer replaces only the bindings set in it while it is active.

use dioxus::prelude::*;

use crate::components::button::Button;
use crate::components::button::ButtonSize;
use crate::components::button::ButtonVariant;
use crate::components::input::Input;
use crate::draft;
use crate::draft::Group;
use crate::editor::BindingRow;
use crate::editor::Editor;
use crate::target::Pick;

#[component]
pub fn LayerSection(
    editor: Signal<Editor>,
    lit: Vec<String>,
) -> Element {
    let mut name = use_signal(String::new);
    let guard = editor.read();
    let vocab = guard.vocab.as_ref().unwrap();
    let names = guard.layer_names();
    let picked = guard
        .layer
        .clone()
        .filter(|layer| names.contains(layer))
        .or_else(|| names.first().cloned());
    let sources: Vec<(Group, String)> = vocab
        .buttons
        .iter()
        .map(|s| (Group::Buttons, s.clone()))
        .chain(vocab.axes.iter().map(|s| (Group::Axes, s.clone())))
        .collect();
    drop(guard);
    // The name typed, when no layer has it
    let mut fresh = move || -> Option<String> {
        let wanted = name().trim().to_string();
        let names = editor.read().layer_names();
        if wanted.is_empty() || names.contains(&wanted) {
            let err = if wanted.is_empty() {
                "Name the layer first".to_string()
            } else {
                format!("Layer {wanted} exists")
            };
            editor.write().show_error(&err);
            return None;
        }
        Some(wanted)
    };
    let picked_add = picked.clone();
    let picked_rename = picked.clone();
    let picked_remove = picked.clone();
    rsx! {
        h2 { "Layers" }
        p { class: "note", "A layer replaces only the bindings set here while it is active; bind layer:hold or layer:toggle on a button to switch it." }
        p { class: "inline",
            if names.is_empty() {
                "No layers. "
            } else {
                Pick { options: names.iter().map(|n| (n.clone(), n.clone())).collect::<Vec<_>>(),
                    value: picked.clone().unwrap_or_default(),
                    onchange: move |n: String| editor.write().layer = Some(n) }
            }
            Input { placeholder: "layer name", value: name(),
                oninput: move |e: FormEvent| name.set(e.value()) }
            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm,
                onclick: move |_| {
                    let _ = &picked_add;
                    let Some(wanted) = fresh() else { return };
                    let mut editor = editor.write();
                    editor.profile().layer.entry(wanted.clone()).or_default();
                    editor.layer = Some(wanted);
                },
                "Add layer"
            }
            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, disabled: picked.is_none(),
                onclick: move |_| {
                    let (Some(wanted), Some(from)) = (fresh(), picked_rename.clone()) else { return };
                    let mut editor = editor.write();
                    let profile = editor.profile();
                    if let Some(layer) = profile.layer.remove(&from) {
                        profile.layer.insert(wanted.clone(), layer);
                    }
                    draft::retarget(profile, &from, &wanted);
                    editor.layer = Some(wanted);
                },
                "Rename layer"
            }
            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, disabled: picked.is_none(),
                onclick: move |_| {
                    let Some(gone) = picked_remove.clone() else { return };
                    let mut editor = editor.write();
                    let profile = editor.profile();
                    profile.layer.remove(&gone);
                    draft::untarget(profile, &gone);
                },
                "Delete layer"
            }
        }
        table { class: "layer-bindings",
            if let Some(layer) = picked {
                for (group, source) in sources {
                    BindingRow { key: "{source}", editor, group, source: source.clone(), layer: Some(layer.clone()),
                        lit: lit.contains(&source) }
                }
            }
        }
    }
}
