use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::select;
use dioxus_primitives::select::SelectOptionProps;
use dioxus_primitives::select::SelectProps;

/// The icons' colour: usvg draws them and sees no page variable.
const ICON: &str = "#8a8a8a";

/// The chevron and check marks are inline here; the original takes them from dioxus-icons.
#[component]
pub fn Select<T: Clone + PartialEq + 'static>(props: SelectProps<T>) -> Element {
    let base = attributes!(div { class: "dx-select" });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        select::Select {
            value: props.value,
            default_value: props.default_value,
            on_value_change: props.on_value_change,
            disabled: props.disabled,
            open: props.open,
            default_open: props.default_open,
            on_open_change: props.on_open_change,
            name: props.name,
            roving_loop: props.roving_loop,
            typeahead_timeout: props.typeahead_timeout,
            attributes: merged,
            select::SelectTrigger {
                class: "dx-select-trigger",
                select::SelectValue { placeholder: "" }
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
                {props.children}
            }
        }
    }
}

#[component]
pub fn SelectOption<T: Clone + PartialEq + 'static>(props: SelectOptionProps<T>) -> Element {
    let base = attributes!(div {
        class: "dx-select-option"
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        select::SelectOption::<T> {
            value: props.value,
            text_value: props.text_value,
            disabled: props.disabled,
            id: props.id,
            index: props.index,
            aria_label: props.aria_label,
            aria_roledescription: props.aria_roledescription,
            attributes: merged,
            {props.children}
            select::SelectItemIndicator {
                svg {
                    width: "16", height: "16", view_box: "0 0 24 24",
                    fill: "none", stroke: ICON, stroke_width: "2",
                    stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "M20 6 9 17l-5-5" }
                }
            }
        }
    }
}
