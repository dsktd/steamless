use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::switch;
use dioxus_primitives::switch::SwitchProps;

use crate::components::button::activates;

/// Enter and Space flip a focused switch, as a click does; only the controlled `checked` is
/// known here, so an uncontrolled switch flips by pointer alone.
#[component]
pub fn Switch(props: SwitchProps) -> Element {
    let checked = props.checked;
    let on_checked_change = props.on_checked_change;
    let base = attributes!(button {
        onkeydown: move |event: KeyboardEvent| {
            if let Some(checked) = checked()
                && activates(&event)
            {
                event.prevent_default();
                on_checked_change.call(!checked);
            }
        },
    });
    let merged = merge_attributes(vec![base, props.attributes]);
    rsx! {
        switch::Switch {
            class: "dx-switch",
            checked: props.checked,
            default_checked: props.default_checked,
            disabled: props.disabled,
            required: props.required,
            name: props.name,
            value: props.value,
            on_checked_change: props.on_checked_change,
            attributes: merged,
            switch::SwitchThumb { class: "dx-switch-thumb" }
        }
    }
}
