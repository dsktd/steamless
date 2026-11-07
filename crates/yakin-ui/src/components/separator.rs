use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::separator;
use dioxus_primitives::separator::SeparatorProps;

#[component]
pub fn Separator(props: SeparatorProps) -> Element {
    let base = attributes!(div {
        class: "dx-separator",
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        separator::Separator {
            horizontal: props.horizontal,
            decorative: props.decorative,
            attributes: merged,
            {props.children}
        }
    }
}
