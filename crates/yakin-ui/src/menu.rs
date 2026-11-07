//! The Quick menu section. Arrays replace whole across layers, so the section carries the tag of
//! the layer its value comes from.

use dioxus::prelude::*;
use yakin_core::menu::MenuItemToml;
use yakin_core::menu::MenuToml;

use crate::components::button::Button;
use crate::components::button::ButtonSize;
use crate::components::button::ButtonVariant;
use crate::components::input::Input;
use crate::draft;
use crate::editor::Editor;
use crate::target::Pick;
use crate::target::TargetCell;

const COLUMNS: (u8, u8) = (2, 6);

/// A section header: title, where the value comes from, and a reset to that.
#[component]
pub fn SectionHead(
    title: String,
    mine: bool,
    tag: String,
    reset: Callback<()>,
) -> Element {
    rsx! {
        h2 {
            {title}
            " "
            span { class: "tag", if mine { "yours" } else { {tag} } }
            if mine {
                Button { variant: ButtonVariant::Ghost, size: ButtonSize::IconXs,
                    title: "Reset to what the layers below set", onclick: move |_| reset.call(()), "↺" }
            }
        }
    }
}

/// Up, down and remove for one row of a list.
#[component]
pub fn Moves(
    index: usize,
    len: usize,
    swap: Callback<(usize, usize)>,
    remove: Callback<usize>,
) -> Element {
    rsx! {
        td { class: "moves",
            Button { variant: ButtonVariant::Ghost, size: ButtonSize::IconXs, title: "Move up", disabled: index == 0,
                onclick: move |_| swap.call((index, index - 1)), "↑" }
            Button { variant: ButtonVariant::Ghost, size: ButtonSize::IconXs, title: "Move down", disabled: index + 1 >= len,
                onclick: move |_| swap.call((index, index + 1)), "↓" }
            Button { variant: ButtonVariant::Ghost, size: ButtonSize::IconXs, title: "Remove",
                onclick: move |_| remove.call(index), "−" }
        }
    }
}

/// Change the menu, which exists once it holds something; a menu that holds nothing, which the
/// config would not have, is no `[menu]` at all.
fn edit(
    editor: &mut Editor,
    change: impl FnOnce(&mut MenuToml),
) {
    let menu = editor.draft.menu.get_or_insert_with(MenuToml::default);
    change(menu);
    let had = editor
        .file
        .as_ref()
        .is_some_and(|file| file.effective.menu.is_some());
    if editor
        .draft
        .menu
        .as_ref()
        .is_some_and(|menu| menu.items.is_empty())
        && !had
    {
        editor.draft.menu = None;
    }
}

#[component]
pub fn MenuSection(editor: Signal<Editor>) -> Element {
    let guard = editor.read();
    let vocab = guard.vocab.as_ref().unwrap();
    let file = guard.file.as_ref().unwrap();
    let menu = guard.draft.menu.clone().unwrap_or_default();
    let under = draft::under_layers(file)
        .into_iter()
        .filter_map(|(tag, toml)| toml.menu.clone().map(|menu| (tag, menu)))
        .next_back();
    let mine = guard.draft.menu != under.as_ref().map(|(_, menu)| menu.clone());
    let tag = under
        .as_ref()
        .map_or("baseline".to_string(), |(tag, _)| tag.clone());
    let icons: Vec<(String, String)> = vocab
        .menu_icons
        .iter()
        .map(|i| (i.clone(), i.clone()))
        .collect();
    let errors = guard.row_errors.clone();
    let set = guard.draft.menu.is_some();
    let first_icon = vocab.menu_icons.first().cloned().unwrap_or_default();
    drop(guard);
    let under_ = under.clone();
    let swap = Callback::new(move |(a, b): (usize, usize)| {
        edit(&mut editor.write(), |menu| menu.items.swap(a, b))
    });
    let remove = Callback::new(move |index: usize| {
        edit(&mut editor.write(), |menu| _ = menu.items.remove(index))
    });
    rsx! {
        SectionHead { title: "Quick menu", mine, tag,
            reset: move |()| editor.write().draft.menu = under_.clone().map(|(_, menu)| menu) }
        if !set {
            p { class: "note", "No [menu] set: the built-in menu is shown in game. Adding an item replaces it." }
        }
        p { class: "inline",
            "Columns "
            Input { r#type: "number", min: COLUMNS.0.to_string(), max: COLUMNS.1.to_string(), value: menu.columns.to_string(),
                onchange: move |e: FormEvent| {
                    let n = e.value().trim().parse::<u8>().unwrap_or(MenuToml::default().columns).clamp(COLUMNS.0, COLUMNS.1);
                    edit(&mut editor.write(), |menu| menu.columns = n);
                }
            }
        }
        table { class: "menu-items",
            for (index, item) in menu.items.iter().enumerate() {
                tr { key: "{index}",
                    td {
                        Input { value: item.label.clone(), placeholder: "label",
                            onchange: move |e: FormEvent| edit(&mut editor.write(), |menu| menu.items[index].label = e.value()) }
                    }
                    td {
                        Pick { options: icons.clone(), value: item.icon.clone(),
                            onchange: move |icon: String| edit(&mut editor.write(), |menu| menu.items[index].icon = icon) }
                    }
                    // Any button target but menu:, which the daemon rejects
                    TargetCell { editor, source: "y", target: Some(item.target.clone()),
                        set: move |target: Option<String>| edit(&mut editor.write(), |menu| menu.items[index].target = target.unwrap_or_else(|| "none".to_string())),
                        unset: None, skip: vec!["menu".to_string()] }
                    Moves { index, len: menu.items.len(), swap, remove }
                    td { span { class: "row-error", {errors.get(&format!("menu.items.{index}")).cloned().unwrap_or_default()} } }
                }
            }
        }
        Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm,
            onclick: move |_| edit(&mut editor.write(), |menu| menu.items.push(MenuItemToml {
                label: "New".to_string(), icon: first_icon.clone(), target: "none".to_string() })),
            "Add item"
        }
        div { class: "menu-preview", style: "grid-template-columns: repeat({menu.columns}, 1fr)",
            for item in menu.items.iter() {
                div { title: item.target.clone(), if item.label.is_empty() { "·" } else { {item.label.clone()} } }
            }
        }
    }
}
