use dioxus::prelude::*;
use dioxus_primitives::dioxus_attributes::attributes;
use dioxus_primitives::merge_attributes;
use dioxus_primitives::tabs;
use dioxus_primitives::tabs::TabContentProps;
use dioxus_primitives::tabs::TabListProps;
use dioxus_primitives::tabs::TabTriggerProps;

#[derive(Props, Clone, PartialEq)]
pub struct TabsProps {
    #[props(default)]
    pub class: String,
    pub value: ReadSignal<Option<String>>,
    #[props(default)]
    pub default_value: String,
    #[props(default)]
    pub on_value_change: Callback<String>,
    #[props(default)]
    pub disabled: ReadSignal<bool>,
    #[props(default)]
    pub horizontal: ReadSignal<bool>,
    #[props(default = ReadSignal::new(Signal::new(true)))]
    pub roving_loop: ReadSignal<bool>,
    #[props(default)]
    pub variant: TabsVariant,
    #[props(extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,
    pub children: Element,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum TabsVariant {
    #[default]
    Default,
    Ghost,
}
impl TabsVariant {
    fn to_class(self) -> &'static str {
        match self {
            TabsVariant::Default => "default",
            TabsVariant::Ghost => "ghost",
        }
    }
}

#[component]
pub fn Tabs(props: TabsProps) -> Element {
    let base = attributes!(div {
        class: format!("{} dx-tabs", props.class),
        "data-variant": props.variant.to_class(),
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        tabs::Tabs {
            value: props.value,
            default_value: props.default_value,
            on_value_change: props.on_value_change,
            disabled: props.disabled,
            horizontal: props.horizontal,
            roving_loop: props.roving_loop,
            attributes: merged,
            {props.children}
        }
    }
}

#[component]
pub fn TabList(props: TabListProps) -> Element {
    let base = attributes!(div {
        class: "dx-tabs-list"
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        tabs::TabList { attributes: merged, {props.children} }
    }
}

#[component]
pub fn TabTrigger(props: TabTriggerProps) -> Element {
    let base = attributes!(button {
        class: format!("dx-tabs-trigger {}", props.class.unwrap_or_default())
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        tabs::TabTrigger {
            class: None,
            id: props.id,
            value: props.value,
            index: props.index,
            disabled: props.disabled,
            attributes: merged,
            {props.children}
        }
    }
}

#[component]
pub fn TabContent(props: TabContentProps) -> Element {
    let base = attributes!(div {
        class: format!(
            "{} dx-tabs-content dx-tabs-content-themed",
            props.class.unwrap_or_default()
        )
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        tabs::TabContent {
            class: None,
            value: props.value,
            id: props.id,
            index: props.index,
            attributes: merged,
            {props.children}
        }
    }
}
