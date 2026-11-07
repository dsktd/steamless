//! SelectTrigger component implementation.

use dioxus::prelude::*;

use super::super::context::SelectContext;

/// The props for the [`SelectTrigger`] component
#[derive(Props, Clone, PartialEq)]
pub struct SelectTriggerProps {
    /// Additional attributes for the trigger button
    #[props(extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,

    /// The children to render inside the trigger
    pub children: Element,
}

/// # SelectTrigger
///
/// The trigger button for the [`Select`](super::select::Select) component which controls if the [`SelectList`](super::list::SelectList) is rendered.
///
/// This must be used inside a [`Select`](super::select::Select) component.
///
/// ```rust
/// use dioxus::prelude::*;
/// use dioxus_primitives::select::{
///     Select, SelectGroup, SelectGroupLabel, SelectItemIndicator, SelectList, SelectOption,
///     SelectTrigger, SelectValue,
/// };
/// #[component]
/// fn Demo() -> Element {
///     rsx! {
///         Select::<String> {
///             SelectTrigger {
///                 aria_label: "Select Trigger",
///                 width: "12rem",
///                 SelectValue { placeholder: "Select a fruit..." }
///             }
///             SelectList {
///                 aria_label: "Select Demo",
///                 SelectGroup {
///                     SelectGroupLabel { "Fruits" }
///                     SelectOption::<String> {
///                         index: 0usize,
///                         value: "apple",
///                         "Apple"
///                         SelectItemIndicator { "✔️" }
///                     }
///                     SelectOption::<String> {
///                         index: 1usize,
///                         value: "banana",
///                         "Banana"
///                         SelectItemIndicator { "✔️" }
///                     }
///                 }
///             }
///         }
///     }
/// }
/// ```
///
///
/// ## Styling
///
/// The [`SelectTrigger`] component defines a span with a `data-placeholder` attribute if a placeholder is set.
#[component]
pub fn SelectTrigger(props: SelectTriggerProps) -> Element {
    let mut ctx = use_context::<SelectContext>();
    let open = ctx.selectable.open;

    // The focus comes back here when the list closes: Blitz drops the focus with the list's
    // element and the next Tab would start over from the top of the page (yakin)
    let mut trigger_ref: Signal<Option<std::rc::Rc<MountedData>>> = use_signal(|| None);
    let mut was_open = use_signal(|| false);
    use_effect(move || {
        let open_now = open();
        if !open_now && *was_open.peek() {
            if let Some(trigger) = trigger_ref.peek().clone() {
                spawn(async move {
                    _ = trigger.set_focus(true).await;
                });
            }
        }
        was_open.set(open_now);
    });

    rsx! {
        button {
            // Standard HTML attributes
            disabled: (ctx.selectable.disabled)(),
            type: "button",
            onmounted: move |evt| trigger_ref.set(Some(evt.data())),

            onclick: move |_| {
                ctx.set_open(!open());
            },
            onkeydown: move |event| {
                match event.key() {
                    // Blitz makes no click out of Enter or Space on a button (yakin)
                    Key::Named(NamedKey::Enter) => {
                        ctx.set_open(!open());
                        event.prevent_default();
                        event.stop_propagation();
                    }
                    Key::Character(c) if c == " " => {
                        ctx.set_open(!open());
                        event.prevent_default();
                        event.stop_propagation();
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        ctx.set_open(true);
                        ctx.selectable
                            .initial_focus
                            .set(ctx.selectable.collection.last_available_index());
                        event.prevent_default();
                        event.stop_propagation();
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        ctx.set_open(true);
                        ctx.selectable
                            .initial_focus
                            .set(ctx.selectable.collection.first_available_index());
                        event.prevent_default();
                        event.stop_propagation();
                    }
                    _ => {}
                }
            },

            // ARIA attributes
            aria_haspopup: "listbox",
            aria_expanded: open(),
            aria_controls: ctx.selectable.list_id,

            // Pass through other attributes
            ..props.attributes,

            // Render children (options)
            {props.children}
        }
    }
}
