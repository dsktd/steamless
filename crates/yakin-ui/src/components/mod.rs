//! The shadcn-style wrappers of dioxus-primitives, copied from DioxusLabs/components
//! (preview/src/components, MIT or Apache-2.0, see LICENSE-MIT here) at the pinned revision.
//! `css_module` needs the dx asset pipeline, so the class names are plain strings and the
//! stylesheets are baked into the page's head (CSS).

pub mod button;
pub mod input;
pub mod label;
pub mod select;
pub mod separator;
pub mod slider;
pub mod switch;
pub mod tabs;

/// Every component's stylesheet after the theme, in one `<style>`.
pub const CSS: &str = concat!(
    include_str!("theme.css"),
    include_str!("button.css"),
    include_str!("input.css"),
    include_str!("label.css"),
    include_str!("select.css"),
    include_str!("separator.css"),
    include_str!("slider.css"),
    include_str!("switch.css"),
    include_str!("tabs.css"),
);
