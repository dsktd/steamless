use dioxus::prelude::*;
use dioxus_primitives::label;
use dioxus_primitives::label::LabelProps;

#[component]
pub fn Label(props: LabelProps) -> Element {
    rsx! {
        label::Label {
            class: "dx-label",
            html_for: props.html_for,
            attributes: props.attributes,
            {props.children}
        }
    }
}
