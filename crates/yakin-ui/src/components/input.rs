use dioxus::prelude::*;

use crate::components::button::activates;

/// `onchange` fires with the last input when the field loses focus or takes Enter, as the
/// browser's would; Blitz only reports the input itself.
#[component]
pub fn Input(
    oninput: Option<EventHandler<FormEvent>>,
    onchange: Option<EventHandler<FormEvent>>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    #[props(extends=GlobalAttributes)]
    #[props(extends=input)]
    attributes: Vec<Attribute>,
) -> Element {
    let mut pending: Signal<Option<FormEvent>> = use_signal(|| None);
    let mut commit = move || {
        if let Some(e) = pending.take()
            && let Some(callback) = onchange
        {
            callback(e);
        }
    };
    rsx! {
        input {
            class: "dx-input",
            oninput: move |e| {
                pending.set(Some(e.clone()));
                _ = oninput.map(|callback| callback(e));
            },
            onblur: move |_| commit(),
            onkeydown: move |e| {
                if activates(&e) {
                    commit();
                }
                _ = onkeydown.map(|callback| callback(e));
            },
            ..attributes,
        }
    }
}
