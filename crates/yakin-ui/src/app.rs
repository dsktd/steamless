//! The window: the sidebar, the Live, Edit and 3D pages, and the status strip.

use dioxus::prelude::*;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;

use crate::components::tabs::TabContent;
use crate::components::tabs::TabList;
use crate::components::tabs::TabTrigger;
use crate::components::tabs::Tabs;
use crate::daemon::Daemon;
use crate::drawing::Drawing;
use crate::editor::Editor;
use crate::editor::EditorPage;
use crate::live::Live;
use crate::model::ModelPage;
use crate::tuning::Sidebar;
use crate::view::Model;

/// The tab the window opens on: "live", "edit" or "model".
#[derive(Clone, Debug, PartialEq)]
pub struct Page(pub String);

/// Every stylesheet, in one `<style>` at the top of the page: the components' after their theme,
/// then the window's.
fn css() -> String {
    format!("{}{}", crate::components::CSS, include_str!("style.css"))
}

fn controller(c: &yakin_ipc::IpcController) -> String {
    let battery = c.battery.map_or("?".to_string(), |b| format!("{b}%"));
    let link = c.link.name();
    let lizard = c.lizard.map_or("?", |on| if on { "on" } else { "off" });
    format!("hidraw{} battery {battery} {link} lizard {lizard}", c.id)
}

#[component]
pub fn App() -> Element {
    let daemon = use_context::<Daemon>();
    let live = use_signal(Live::default);
    let state = use_signal(ControllerState::default);
    let editor = use_signal(Editor::default);
    let start = use_context::<Page>();
    let mut page = use_signal(move || Some(start.0.clone()));
    crate::live::use_follow(daemon, live, state, editor);
    // The 3D scene outlives its tab: the widget is made once, its thread starts at the first paint
    let mut selected = use_signal(|| None::<Button>);
    use_hook(|| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        spawn(async move {
            while let Some(pick) = rx.recv().await {
                selected.set(pick);
            }
        });
        provide_context(Model::new(tx));
    });
    // `just ui-smoke` exits once the page is up; a no-op otherwise
    use_effect(|| {
        if std::env::var_os("YAKIN_SMOKE").is_some()
            && std::env::var_os("YAKIN_SMOKE_RESIZE").is_none()
        {
            std::process::exit(0);
        }
    });
    let css = use_hook(css);
    let guard = live.read();
    let connected = guard.connected;
    let status = guard.status.clone();
    let profile = guard.profile.clone();
    let yielded = guard.yielded.clone();
    let controllers = guard.controllers.clone();
    drop(guard);
    rsx! {
        style { {css} }
        main {
            div { id: "panes",
                Sidebar { live }
                div { id: "view",
                    Tabs { value: page, horizontal: true, on_value_change: move |next: String| page.set(Some(next)),
                        TabList {
                            TabTrigger { value: "live", index: 0usize, "Live" }
                            TabTrigger { value: "edit", index: 1usize, "Edit" }
                            TabTrigger { value: "model", index: 2usize, "3D" }
                        }
                        // Only the open tab's panel is in the page: Blitz tabs to a hidden one
                        if page() == Some("edit".to_string()) {
                            TabContent { value: "edit", index: 1usize, id: "editor",
                                EditorPage { editor, live, state }
                            }
                        } else if page() == Some("model".to_string()) {
                            TabContent { value: "model", index: 2usize, id: "stage",
                                ModelPage { editor, state, selected, page }
                            }
                        } else {
                            TabContent { value: "live", index: 0usize, id: "controller",
                                Drawing { state, yielded: yielded.is_some() }
                            }
                        }
                    }
                }
            }
            footer { id: "status",
                span { id: "connection", class: if connected { "on" } else { "off" },
                    if connected { "connected" } else { {status} }
                }
                if let Some(holder) = yielded {
                    span { id: "yielded", title: "pid {holder.pid}", "yielded to {holder.comm}" }
                }
                span { "profile " b { {profile.unwrap_or_else(|| "?".to_string())} } }
                ul {
                    for c in controllers.into_iter().filter(|c| c.connected) {
                        li { key: "{c.id}", {controller(&c)} }
                    }
                }
            }
        }
    }
}
