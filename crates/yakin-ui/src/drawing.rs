//! The controller drawing with the live input on it. The SVG is baked in and shown with a
//! `<style>` of its own naming what is pressed by element id (Blitz hands an SVG to usvg,
//! which sees none of the page's stylesheet), and the stick, pad and trigger positions are
//! drawn on an overlay in the same coordinate space.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use yakin_core::state::ControllerState;

/// Made from Valve's CAD by `just assets` (see NOTICE), linked into ui/assets.
pub const SVG: &str = include_str!("../../../ui/assets/controller.svg");
const AXIS: f32 = 32767.0;
const TRIGGER: f32 = 32767.0;

/// Element id to bit of the button field (yakin_core::state::Button).
const BUTTONS: [(&str, u32); 22] = [
    ("btn-a", 0),
    ("btn-b", 1),
    ("btn-x", 2),
    ("btn-y", 3),
    ("btn-qam", 4),
    ("rstick", 5),
    ("btn-menu", 6),
    ("grip-r1", 7),
    ("grip-r2", 8),
    ("rb", 9),
    ("dpad-down", 10),
    ("dpad-right", 11),
    ("dpad-left", 12),
    ("dpad-up", 13),
    ("btn-view", 14),
    ("lstick", 15),
    ("btn-steam", 16),
    ("grip-l1", 17),
    ("grip-l2", 18),
    ("lb", 19),
    ("rpad", 22),
    ("lpad", 26),
];
/// Element id to bit of a touch sensor: a lighter highlight than a press.
const TOUCH: [(&str, u32); 4] = [
    ("rstick", 20),
    ("lstick", 24),
    ("grip-r-touch", 28),
    ("grip-l-touch", 29),
];

/// The colours drawn into the SVG, where the page's variables do not reach: lines, pressed
/// and touched controls, the stick and pad dots.
const LINE: &str = "#8a8a8a";
pub const ON: &str = "#4fa3ff";
pub const DOT: &str = "#ffb347";

/// The drawing with its stylesheet: outlines, and the lit elements filled by id.
pub fn svg(
    on: &[&str],
    touch: &[&str],
) -> String {
    let mut style = format!(
        "<style>* {{ fill: none; stroke: {LINE}; stroke-width: 2 }} .back {{ stroke-dasharray: 6 4 }}"
    );
    for id in on {
        style.push_str(&format!(
            " #{id} {{ fill: {ON}; fill-opacity: 0.6; stroke: {ON} }}"
        ));
    }
    for id in touch {
        style.push_str(&format!(" #{id} {{ fill: {ON}; fill-opacity: 0.2 }}"));
    }
    style.push_str("</style>");
    let head = SVG.find('>').map_or(0, |at| at + 1);
    format!("{}{style}{}", &SVG[..head], &SVG[head..])
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Bbox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The bounding boxes of the elements the overlay draws on, read from the SVG text: the
/// controls are polylines (`M x,y L x,y ...`), the triggers rects.
pub fn boxes() -> BTreeMap<String, Bbox> {
    let mut boxes = BTreeMap::new();
    for id in ["lpad", "rpad", "lstick", "rstick", "lt", "rt"] {
        let Some(start) = SVG.find(&format!("id=\"{id}\"")) else {
            continue;
        };
        let Some(end) = SVG[start..].find("/>").or_else(|| SVG[start..].find('>')) else {
            continue;
        };
        let tag = &SVG[start..start + end];
        if let Some(bbox) = rect(tag).or_else(|| path(tag)) {
            boxes.insert(id.to_string(), bbox);
        }
    }
    boxes
}

fn attr(
    tag: &str,
    name: &str,
) -> Option<String> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let end = tag[start..].find('"')?;
    Some(tag[start..start + end].to_string())
}

fn rect(tag: &str) -> Option<Bbox> {
    let number = |name: &str| attr(tag, name)?.parse::<f32>().ok();
    Some(Bbox {
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
    })
}

fn path(tag: &str) -> Option<Bbox> {
    let d = attr(tag, "d")?;
    let mut min = (f32::MAX, f32::MAX);
    let mut max = (f32::MIN, f32::MIN);
    for pair in d.split_whitespace() {
        let pair = pair.trim_start_matches(['M', 'L', 'Z']);
        let Some((x, y)) = pair.split_once(',') else {
            continue;
        };
        let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) else {
            continue;
        };
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    }
    (min.0 < max.0).then_some(Bbox {
        x: min.0,
        y: min.1,
        width: max.0 - min.0,
        height: max.1 - min.1,
    })
}

/// The element ids `state` presses and touches.
pub fn lit(state: &ControllerState) -> (Vec<&'static str>, Vec<&'static str>) {
    let bits = state.buttons.bits();
    let lit = |table: &[(&'static str, u32)]| -> Vec<&'static str> {
        table
            .iter()
            .filter(|(_, bit)| (bits >> bit) & 1 == 1)
            .map(|(id, _)| *id)
            .collect()
    };
    (lit(&BUTTONS), lit(&TOUCH))
}

/// A dot in the element's box at a stick or pad position (y up), as the overlay draws it.
fn dot(
    bbox: &Bbox,
    x: i16,
    y: i16,
) -> (f32, f32, f32) {
    let cx = bbox.x + (x as f32 + AXIS) / (2.0 * AXIS) * bbox.width;
    let cy = bbox.y + (1.0 - (y as f32 + AXIS) / (2.0 * AXIS)) * bbox.height;
    (cx, cy, (bbox.width.min(bbox.height) / 10.0).max(4.0))
}

#[component]
pub fn Drawing(
    state: ReadSignal<ControllerState>,
    yielded: bool,
) -> Element {
    let boxes = use_hook(boxes);
    // The SVG is parsed again only when what is lit changes
    let lit = use_memo(move || lit(&state.read()));
    let drawing = use_memo(move || {
        let (on, touch) = lit();
        svg(&on, &touch)
    });
    let state = state.read();
    let dots = [
        ("lpad", state.lpad.x, state.lpad.y, state.lpad.touched),
        ("rpad", state.rpad.x, state.rpad.y, state.rpad.touched),
        ("lstick", state.lstick.x, state.lstick.y, true),
        ("rstick", state.rstick.x, state.rstick.y, true),
    ];
    let fills = [("lt", state.lt), ("rt", state.rt)];
    rsx! {
        div {
            id: "drawing",
            class: if yielded { "yielded" },
            div { class: "layer", dangerous_inner_html: drawing() }
            svg { class: "layer overlay", view_box: "0 0 1000 745",
                for (id, x, y, shown) in dots {
                    if shown {
                        if let Some(bbox) = boxes.get(id) {
                            {
                                let (cx, cy, r) = dot(bbox, x, y);
                                rsx! { circle { cx, cy, r, fill: DOT } }
                            }
                        }
                    }
                }
                for (id, value) in fills {
                    if let Some(bbox) = boxes.get(id) {
                        {
                            let height = (value as f32).min(TRIGGER) / TRIGGER * bbox.height;
                            rsx! {
                                if height > 0.0 {
                                    rect { x: bbox.x, width: bbox.width, rx: 14, fill: ON, fill_opacity: 0.5,
                                        y: bbox.y + bbox.height - height, height }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
