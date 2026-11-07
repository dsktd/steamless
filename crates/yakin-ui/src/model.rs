//! The 3D page: the controller to orbit and click, with the clicked control's bindings beside
//! it, edited in the same draft as the Edit page. With a controller alone: the D-pad's arrows
//! walk the control list, Enter (A) edits the picked control's binding, Escape (B) backs out.

use blitz_dom::NodeId;
use dioxus::prelude::*;
use dioxus_native::NodeHandle;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;
use yakin_view::layout;

use crate::components::button::Button as Push;
use crate::components::button::ButtonSize;
use crate::components::button::ButtonVariant;
use crate::daemon::Daemon;
use crate::draft::Group;
use crate::editor::BindingRow;
use crate::editor::Editor;
use crate::view::Model;
use crate::view::Request;

/// Every control on the model, in the order the panel lists them.
pub fn controls() -> Vec<Button> {
    layout::ZONES
        .iter()
        .map(|zone| zone.button)
        .chain(layout::STICKS.iter().map(|(_, button, _)| *button))
        .chain(layout::TRIGGERS.iter().map(|(_, button, _)| *button))
        .collect()
}

/// What a key does on the control list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Nav {
    Previous,
    Next,
    Edit,
    Back,
    /// Off the list, to the tab on this side.
    Tab(bool),
}
impl Nav {
    pub fn from_key(key: &Key) -> Option<Self> {
        match key {
            Key::Named(NamedKey::ArrowUp) => Some(Self::Previous),
            Key::Named(NamedKey::ArrowDown) => Some(Self::Next),
            Key::Named(NamedKey::Enter) => Some(Self::Edit),
            Key::Named(NamedKey::Escape) => Some(Self::Back),
            Key::Named(NamedKey::ArrowLeft) => Some(Self::Tab(false)),
            Key::Named(NamedKey::ArrowRight) => Some(Self::Tab(true)),
            _ => None,
        }
    }
}

/// The control picked after a step through `list` from `selected`; it stops at either end, and
/// from none the first step takes the first or the last.
pub fn step<T: Copy + PartialEq>(
    list: &[T],
    selected: Option<T>,
    next: bool,
) -> Option<T> {
    let last = list.len().checked_sub(1)?;
    let at = selected.and_then(|button| list.iter().position(|b| *b == button));
    let index = match (at, next) {
        (None, true) => 0,
        (None, false) => last,
        (Some(at), true) => (at + 1).min(last),
        (Some(at), false) => at.saturating_sub(1),
    };
    Some(list[index])
}

/// The tab pages, left to right, as the tab bar lists them.
pub const PAGES: [&str; 3] = ["live", "edit", "model"];

/// The page a step left or right of `page` opens; like the tab bar, it wraps round.
pub fn tab_step(
    page: &str,
    next: bool,
) -> Option<&'static str> {
    let at = PAGES.iter().position(|name| *name == page)?;
    let len = PAGES.len();
    Some(
        PAGES[if next {
            (at + 1) % len
        } else {
            (at + len - 1) % len
        }],
    )
}

/// Moves the focus one step among the focusable nodes under the handle's node, stopping at the ends. A
/// focus outside them goes to the first or the last.
fn walk(
    handle: &NodeHandle,
    selector: &str,
    next: bool,
) {
    let mut doc = handle.doc_mut();
    let Ok(found) = doc.query_selector_all_in(handle.node_id(), selector) else {
        return;
    };
    let list: Vec<NodeId> = found
        .into_iter()
        .filter(|id| doc.get_node(*id).is_some_and(|node| node.is_focussable()))
        .collect();
    if let Some(to) = step(&list, doc.get_focussed_node_id(), next) {
        doc.set_focus_to(to);
    }
}

/// Focuses the tab button of `page`.
fn focus_tab(
    handle: &NodeHandle,
    page: &str,
) {
    let mut doc = handle.doc_mut();
    let Some(at) = PAGES.iter().position(|name| *name == page) else {
        return;
    };
    let tabs = doc.query_selector_all("[role=tab]").unwrap_or_default();
    if let Some(tab) = tabs.get(at) {
        doc.set_focus_to(*tab);
    }
}

#[component]
pub fn ModelPage(
    editor: Signal<Editor>,
    state: Signal<ControllerState>,
    selected: Signal<Option<Button>>,
    page: Signal<Option<String>>,
) -> Element {
    let daemon = use_context::<Daemon>();
    let model = use_context::<Model>();
    // Every state the daemon streams goes to the scene
    let model_ = model.clone();
    use_effect(move || model_.send(Request::State(state())));
    let buttons = use_memo(move || {
        editor
            .read()
            .vocab
            .as_ref()
            .map(|vocab| vocab.buttons.clone())
            .unwrap_or_default()
    });
    let lit = use_memo(move || crate::live::lit(&state.read(), &buttons.read()));
    let guard = editor.read();
    let ready = guard.vocab.is_some() && guard.file.is_some();
    let editing = guard.editing();
    let changed = guard.dirty();
    let error = guard.error.clone();
    drop(guard);
    let daemon_ = daemon.clone();
    // Editing hides the list, so going back mounts it again and it takes the focus
    let mut editing_row = use_signal(|| false);
    let list = use_hook(controls);
    let list_ = list.clone();
    let model_ = model.clone();
    let pick = use_callback(move |button: Option<Button>| {
        selected.set(button);
        model_.send(Request::Select(button));
    });
    let mut panel = use_signal(|| None::<NodeHandle>);
    let on_list_key = move |event: KeyboardEvent| {
        let Some(nav) = Nav::from_key(&event.key()) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        match nav {
            Nav::Previous => pick.call(step(&list_, selected(), false)),
            Nav::Next => pick.call(step(&list_, selected(), true)),
            Nav::Edit if selected().is_some() && ready => editing_row.set(true),
            Nav::Edit => {}
            Nav::Back => pick.call(None),
            Nav::Tab(next) => {
                let current = page().unwrap_or_default();
                let (Some(to), Some(handle)) = (tab_step(&current, next), panel()) else {
                    return;
                };
                focus_tab(&handle, to);
                page.set(Some(to.to_string()));
            }
        }
    };
    let on_row_key = move |event: KeyboardEvent| {
        if !editing_row() || !event.default_action_enabled() {
            return;
        }
        // An open select takes its own arrows and Escape first
        match Nav::from_key(&event.key()) {
            Some(Nav::Back) => editing_row.set(false),
            Some(Nav::Previous | Nav::Next) => {
                if let Some(handle) = panel() {
                    walk(&handle, "*", Nav::from_key(&event.key()) == Some(Nav::Next));
                }
            }
            _ => return,
        }
        event.prevent_default();
        event.stop_propagation();
    };
    let edit = editing_row() && selected().is_some();
    rsx! {
        div { id: "model",
            object { id: "scene", "data": model.widget.clone() }
            aside { id: "binding", class: "editor", onkeydown: on_row_key,
                onmounted: move |event| panel.set(event.data().downcast::<NodeHandle>().cloned()),
                if !edit {
                    ul { id: "controls", tabindex: 0, autofocus: true, onkeydown: on_list_key,
                        for button in list.iter().copied() {
                            li { key: "{button}", class: if selected() == Some(button) { "picked" },
                                onclick: move |_| pick.call(Some(button)),
                                {button.name()}
                            }
                        }
                    }
                }
                match selected() {
                    None => rsx! {
                        p { class: "note", "Pick a control on the model or in the list (D-pad) to see and change its bindings." }
                        p { class: "note", "Drag or the right stick turns the controller, scroll or the triggers zoom." }
                    },
                    Some(button) => rsx! {
                        h2 { {button.name()} }
                        if edit {
                            p { class: "note", "B (Escape) goes back to the list." }
                        } else if ready {
                            p { class: "note", "A (Enter) edits the binding." }
                        }
                        p { class: "note", "profile {editing}" }
                        if ready {
                            table { class: "bindings",
                                BindingRow { key: "{button}-{edit}", editor, group: Group::Buttons, source: button.name().to_string(),
                                    layer: None, focus: edit, lit: lit.read().iter().any(|name| name == button.name()) }
                                if let Some(axis) = layout::axis(button) {
                                    BindingRow { key: "{axis.name()}", editor, group: Group::Axes, source: axis.name().to_string(),
                                        layer: None, lit: lit.read().iter().any(|name| name == axis.name()) }
                                }
                            }
                            div { class: "bar",
                                span { class: "note", if changed { "unsaved" } }
                                span { class: "spacer" }
                                Push { variant: ButtonVariant::Outline, size: ButtonSize::Sm,
                                    onclick: move |_| editor.write().revert(&daemon_),
                                    "Revert all"
                                }
                                Push { size: ButtonSize::Sm, disabled: !changed,
                                    // The whole draft, the Edit page's changes too
                                    onclick: move |_| editor.write().save(&daemon),
                                    "Save all"
                                }
                            }
                            p { class: "row-error", {error} }
                        } else {
                            p { class: "note", "No config from the daemon yet." }
                            p { class: "row-error", {error} }
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_controls() {
        let list = controls();
        assert_eq!(list.len(), 24);
        assert_eq!(list.first(), Some(&Button::A));
        assert!(list.contains(&Button::RStickClick) && list.contains(&Button::LtFull));
    }

    #[test]
    fn test_step() {
        let list = [Button::A, Button::B, Button::X];
        assert_eq!(step(&list, None, true), Some(Button::A));
        assert_eq!(step(&list, None, false), Some(Button::X));
        assert_eq!(step(&list, Some(Button::A), true), Some(Button::B));
        assert_eq!(step(&list, Some(Button::B), false), Some(Button::A));
        assert_eq!(
            step(&list, Some(Button::X), true),
            Some(Button::X),
            "stops at the end"
        );
        assert_eq!(
            step(&list, Some(Button::A), false),
            Some(Button::A),
            "stops at the start"
        );
        assert_eq!(
            step(&list, Some(Button::Steam), true),
            Some(Button::A),
            "not listed: from none"
        );
        assert_eq!(step::<Button>(&[], None, true), None);
    }

    #[test]
    fn test_nav() {
        assert_eq!(
            Nav::from_key(&Key::Named(NamedKey::ArrowUp)),
            Some(Nav::Previous)
        );
        assert_eq!(
            Nav::from_key(&Key::Named(NamedKey::ArrowDown)),
            Some(Nav::Next)
        );
        assert_eq!(Nav::from_key(&Key::Named(NamedKey::Enter)), Some(Nav::Edit));
        assert_eq!(
            Nav::from_key(&Key::Named(NamedKey::Escape)),
            Some(Nav::Back)
        );
        assert_eq!(
            Nav::from_key(&Key::Named(NamedKey::ArrowLeft)),
            Some(Nav::Tab(false))
        );
        assert_eq!(Nav::from_key(&Key::Named(NamedKey::Tab)), None);
    }

    #[test]
    fn test_tab_step() {
        assert_eq!(tab_step("model", false), Some("edit"));
        assert_eq!(tab_step("live", false), Some("model"), "wraps");
        assert_eq!(tab_step("model", true), Some("live"), "wraps");
        assert_eq!(tab_step("edit", true), Some("model"));
        assert_eq!(tab_step("nope", true), None);
    }

    #[test]
    fn test_step_fields() {
        let fields = [10usize, 11, 12];
        assert_eq!(step(&fields, Some(11), true), Some(12));
        assert_eq!(step(&fields, Some(12), true), Some(12), "stops at Save");
        assert_eq!(
            step(&fields, Some(99), true),
            Some(10),
            "outside: the first"
        );
        assert_eq!(
            step(&fields, Some(99), false),
            Some(12),
            "outside: the last"
        );
    }
}
