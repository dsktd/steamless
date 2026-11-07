//! The sidebar: profile buttons, the active profile's tuning knobs, and Reload. Changes apply
//! live and the daemon saves them to the editor drop-in a second after the last one.

use std::time::Instant;

use dioxus::prelude::*;
use yakin_ipc::IpcTuning;

use crate::components::button::Button;
use crate::components::button::ButtonSize;
use crate::components::button::ButtonVariant;
use crate::components::slider::Slider;
use crate::components::switch::Switch;
use crate::daemon::Daemon;
use crate::live::Live;
use crate::live::THROTTLE;

/// A slider knob: field, label, min, max, step.
const KNOBS: [(&str, &str, f64, f64, f64); 15] = [
    ("mouse_speed", "Mouse speed", 0.2, 3.0, 0.05),
    ("mouse_accel", "Mouse accel", 0.0, 3.0, 0.05),
    ("mouse_release_guard", "Release guard", 0.0, 10.0, 1.0),
    ("mouse_smoothing", "Smoothing", 0.0, 1.0, 0.05),
    (
        "mouse_stick_speed",
        "Stick cursor speed",
        100.0,
        4000.0,
        50.0,
    ),
    ("mouse_stick_curve", "Stick curve", 1.0, 3.0, 0.05),
    (
        "mouse_flick_degrees",
        "Flick px per turn",
        500.0,
        8000.0,
        50.0,
    ),
    ("scroll_speed", "Scroll speed", 0.2, 3.0, 0.05),
    ("scroll_stick_speed", "Stick scroll speed", 1.0, 40.0, 1.0),
    ("deadzone_stick", "Stick deadzone", 0.0, 0.5, 0.01),
    ("deadzone_trigger", "Trigger deadzone", 0.0, 0.5, 0.01),
    ("trigger_click", "Trigger click", 0.05, 0.95, 0.05),
    ("haptics_click", "Click pulse", 0.0, 1.0, 0.05),
    ("haptics_pad_ticks", "Pad ticks", 0.0, 1.0, 0.05),
    ("haptics_menu", "Menu ticks", 0.0, 1.0, 0.05),
];

fn knob(
    tuning: &IpcTuning,
    field: &str,
) -> f64 {
    let value = match field {
        "mouse_speed" => tuning.mouse_speed,
        "mouse_accel" => tuning.mouse_accel,
        "mouse_release_guard" => tuning.mouse_release_guard as f32,
        "mouse_smoothing" => tuning.mouse_smoothing,
        "mouse_stick_speed" => tuning.mouse_stick_speed,
        "mouse_stick_curve" => tuning.mouse_stick_curve,
        "mouse_flick_degrees" => tuning.mouse_flick_degrees,
        "scroll_speed" => tuning.scroll_speed,
        "scroll_stick_speed" => tuning.scroll_stick_speed,
        "deadzone_stick" => tuning.deadzone_stick,
        "deadzone_trigger" => tuning.deadzone_trigger,
        "trigger_click" => tuning.trigger_click,
        "haptics_click" => tuning.haptics_click,
        "haptics_pad_ticks" => tuning.haptics_pad_ticks,
        "haptics_menu" => tuning.haptics_menu,
        _ => unreachable!(),
    };
    value as f64
}

fn set_knob(
    tuning: &mut IpcTuning,
    field: &str,
    value: f64,
) {
    let value = value as f32;
    match field {
        "mouse_speed" => tuning.mouse_speed = value,
        "mouse_accel" => tuning.mouse_accel = value,
        "mouse_release_guard" => tuning.mouse_release_guard = value.round() as u8,
        "mouse_smoothing" => tuning.mouse_smoothing = value,
        "mouse_stick_speed" => tuning.mouse_stick_speed = value,
        "mouse_stick_curve" => tuning.mouse_stick_curve = value,
        "mouse_flick_degrees" => tuning.mouse_flick_degrees = value,
        "scroll_speed" => tuning.scroll_speed = value,
        "scroll_stick_speed" => tuning.scroll_stick_speed = value,
        "deadzone_stick" => tuning.deadzone_stick = value,
        "deadzone_trigger" => tuning.deadzone_trigger = value,
        "trigger_click" => tuning.trigger_click = value,
        "haptics_click" => tuning.haptics_click = value,
        "haptics_pad_ticks" => tuning.haptics_pad_ticks = value,
        "haptics_menu" => tuning.haptics_menu = value,
        _ => unreachable!(),
    }
}

/// The value as the sidebar shows it; reports arrive about every 10 ms.
fn shown(
    field: &str,
    value: f64,
) -> String {
    match field {
        "mouse_release_guard" => format!("{value} reports (~{} ms)", value * 10.0),
        "mouse_stick_speed" => format!("{value} px/s"),
        "mouse_flick_degrees" => format!("{value} px"),
        "scroll_stick_speed" => format!("{value} notches/s"),
        _ => format!("{value:.2}"),
    }
}

#[component]
pub fn Sidebar(live: Signal<Live>) -> Element {
    let daemon = use_context::<Daemon>();
    // When the last tune request went out, for the throttle
    let mut last = use_signal(|| Instant::now() - THROTTLE);
    let guard = live.read();
    let active = guard.profile.clone();
    let names = guard.names.clone();
    let tuning = active
        .as_deref()
        .and_then(|name| guard.tuning(name))
        .cloned();
    let error = guard.error.clone();
    drop(guard);
    // Leading and trailing edge: the first move goes out at once, the last one always goes out
    let daemon_ = daemon.clone();
    let send = move |change: &dyn Fn(&mut IpcTuning)| {
        let mut guard = live.write();
        let Some(name) = guard.profile.clone() else {
            return;
        };
        let Some(current) = guard.tunings.iter_mut().find(|t| t.profile == name) else {
            return;
        };
        change(current);
        if guard.tuning_pending {
            return;
        }
        guard.tuning_pending = true;
        drop(guard);
        let wait = THROTTLE.saturating_sub(last().elapsed());
        let daemon = daemon_.clone();
        spawn(async move {
            tokio::time::sleep(wait).await;
            last.set(Instant::now());
            let tuning = {
                let mut guard = live.write();
                guard.tuning_pending = false;
                guard
                    .profile
                    .clone()
                    .and_then(|name| guard.tuning(&name).cloned())
            };
            if let Some(tuning) = tuning {
                let result = daemon.tune(tuning);
                live.write().report(result);
            }
        });
    };
    let daemon_reload = daemon.clone();
    rsx! {
        aside { id: "side",
            h2 { "Profiles" }
            div { id: "profiles",
                for name in names {
                    {
                        let daemon = daemon.clone();
                        let name_ = name.clone();
                        let on = active.as_deref() == Some(name.as_str());
                        rsx! {
                            Button { key: "{name}", size: ButtonSize::Sm,
                                variant: if on { ButtonVariant::Primary } else { ButtonVariant::Outline },
                                onclick: move |_| {
                                    let result = daemon.activate(&name_);
                                    live.write().report(result);
                                },
                                {name.clone()}
                            }
                        }
                    }
                }
            }
            h2 { "Tuning " span { {active.clone().unwrap_or_default()} } }
            div { id: "tuning",
                for (field, label, min, max, step) in KNOBS {
                    {
                        let value = tuning.as_ref().map(|t| knob(t, field));
                        let mut send = send.clone();
                        rsx! {
                            div { class: "knob", key: "{field}",
                                div { class: "knob-head",
                                    span { {label} }
                                    output { {value.map(|v| shown(field, v)).unwrap_or_default()} }
                                }
                                Slider { value, min, max, step, disabled: tuning.is_none(),
                                    label: Some(label.to_string()),
                                    on_value_change: move |v: f64| send(&|t| set_knob(t, field, v)) }
                            }
                        }
                    }
                }
                label { class: "check",
                    Switch { checked: tuning.as_ref().map(|t| t.scroll_natural), disabled: tuning.is_none(),
                        on_checked_change: {
                            let mut send = send.clone();
                            move |on: bool| send(&|t| t.scroll_natural = on)
                        } }
                    " Natural scroll"
                }
            }
            p { class: "note", "Changes apply live and are saved to the editor drop-in a second after the last one." }
            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm,
                onclick: move |_| {
                    let result = daemon_reload.reload();
                    live.write().report(result);
                },
                "Reload"
            }
            p { id: "error", {error} }
        }
    }
}
