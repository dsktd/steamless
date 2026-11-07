//! The editor's model driven as a user would, against tests/fixtures: the daemon's own
//! Vocabulary and GetConfig for examples/ (a daemon test fails when stale; `just ui-fixtures`
//! rewrites them).

use yakin_core::profile::ActivatorsToml;
use yakin_core::profile::BindingToml;
use yakin_core::profile::ConfigToml;
use yakin_core::profile::Vocabulary;
use yakin_ipc::IpcConfig;
use yakin_ipc::IpcLayer;
use yakin_ui::draft;
use yakin_ui::draft::Group;
use yakin_ui::draft::Kinds;
use yakin_ui::editor::Editor;

fn vocabulary() -> Vocabulary {
    serde_json::from_str(include_str!("fixtures/vocabulary.json")).unwrap()
}

/// `IpcConfig` only serializes (the daemon sends it in bincode), so it is put together here.
fn config() -> IpcConfig {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/config.json")).unwrap();
    let layer = |value: &serde_json::Value| IpcLayer {
        path: value["path"].as_str().unwrap().to_string(),
        toml: serde_json::from_value::<ConfigToml>(value["toml"].clone()).unwrap(),
    };
    IpcConfig {
        baseline: json["baseline"]
            .as_object()
            .map(|_| layer(&json["baseline"])),
        dropins: json["dropins"]
            .as_array()
            .unwrap()
            .iter()
            .map(layer)
            .collect(),
        effective: serde_json::from_value(json["effective"].clone()).unwrap(),
        editor_path: json["editor_path"].as_str().unwrap().to_string(),
        writable: json["writable"].as_bool().unwrap(),
    }
}

fn editor() -> Editor {
    let mut editor = Editor {
        vocab: Some(vocabulary()),
        ..Default::default()
    };
    editor.set_file(config(), false);
    editor.editing = Some("desktop".to_string());
    editor
}

/// The dotted paths whose value differs between two configs' TOML, top-level tables as one.
fn changed(
    before: &ConfigToml,
    after: &ConfigToml,
) -> Vec<String> {
    let mut paths = Vec::new();
    for (name, profile) in &after.profile {
        let was = before.profile.get(name);
        if was != Some(profile) {
            for (button, binding) in &profile.buttons {
                if was.and_then(|p| p.buttons.get(button)) != Some(binding) {
                    paths.push(format!("profile.{name}.buttons.{button}"));
                }
            }
            for (axis, target) in &profile.axes {
                if was.and_then(|p| p.axes.get(axis)) != Some(target) {
                    paths.push(format!("profile.{name}.axes.{axis}"));
                }
            }
            if was.map(|p| &p.layer) != Some(&profile.layer) {
                paths.push(format!("profile.{name}.layer"));
            }
            if was.map(|p| &p.gyro) != Some(&profile.gyro) {
                paths.push(format!("profile.{name}.gyro"));
            }
            if was.map(|p| &p.chord) != Some(&profile.chord) {
                paths.push(format!("profile.{name}.chord"));
            }
        }
    }
    if before.menu != after.menu {
        paths.push("menu".to_string());
    }
    if before.auto != after.auto {
        paths.push("auto".to_string());
    }
    paths
}

#[test]
fn test_targets_round_trip() {
    let kinds = Kinds::new(&vocabulary(), &["nav".to_string()]);
    for target in [
        "key:KEY_LEFTALT+KEY_TAB",
        "keys:KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT",
        "flicks:desktop:focus-up,desktop:focus-down,desktop:focus-left,desktop:focus-right",
        "desktop:spawn:sh -c 'a, b'",
        "desktop:workspace:3",
        "mouse:left",
        "profile:browser",
        "layer:hold:nav",
        "cursor",
    ] {
        let (kind, args) = kinds.parse(target);
        let kind = kinds.named(&kind.unwrap()).unwrap();
        assert_eq!(draft::build(kind, &args), target, "{target}");
    }
    assert_eq!(kinds.parse("pad:south").0.as_deref(), Some("button"));
    assert_eq!(kinds.parse("pad:left").0.as_deref(), Some("stick"));
    assert_eq!(kinds.parse("nonsense:x").0, None);
}

#[test]
fn test_layer_kinds_offered_with_layers_only() {
    let vocab = vocabulary();
    let none = Kinds::new(&vocab, &[]);
    assert!(
        !none
            .allowed(&vocab, "a", None)
            .iter()
            .any(|k| k == "layer:hold")
    );
    // Still offered to the binding that already is one, so it can be shown and changed
    assert!(
        none.allowed(&vocab, "a", Some("layer:hold"))
            .iter()
            .any(|k| k == "layer:hold")
    );
    let some = Kinds::new(&vocab, &["nav".to_string()]);
    assert!(
        some.allowed(&vocab, "a", None)
            .iter()
            .any(|k| k == "layer:toggle")
    );
    assert!(
        !some
            .allowed(&vocab, "lstick", None)
            .iter()
            .any(|k| k == "layer:toggle")
    );
}

#[test]
fn test_slots_pack_shortest() {
    assert_eq!(draft::pack(draft::slots(None)), None);
    let press = BindingToml::Target("pad:a".to_string());
    assert_eq!(draft::pack(draft::slots(Some(&press))), Some(press.clone()));
    let mut slots = draft::slots(Some(&press));
    draft::set_slot(&mut slots, "long", Some("mouse:right".to_string()));
    let packed = draft::pack(slots.clone()).unwrap();
    assert!(
        matches!(&packed, BindingToml::Activators(a) if a.long.as_deref() == Some("mouse:right"))
    );
    assert_eq!(draft::extra(&slots), 1);
    draft::set_slot(&mut slots, "long", None);
    assert_eq!(draft::pack(slots), Some(press));
    let empty = ActivatorsToml {
        press: Some(String::new()),
        ..Default::default()
    };
    assert_eq!(draft::pack(empty), None);
}

#[test]
fn test_binding_changes_one_key() {
    let mut editor = editor();
    assert!(!editor.dirty());
    let file = editor.file.clone().unwrap();
    // The row's tag names the layer that sets it; the drop-in overrides the baseline
    let (_, tag) = draft::under_binding(&file, "desktop", None, Group::Buttons, "a");
    assert_eq!(tag, "baseline");
    draft::set_binding(
        editor.profile(),
        None,
        Group::Buttons,
        "a",
        Some("mouse:left".into()),
    );
    assert!(editor.dirty());
    assert_eq!(
        changed(&file.effective, &editor.draft),
        ["profile.desktop.buttons.a"]
    );
    // Reset to what the layers under set it
    let (under, _) = draft::under_binding(&file, "desktop", None, Group::Buttons, "a");
    draft::set_binding(editor.profile(), None, Group::Buttons, "a", under);
    assert!(!editor.dirty());
}

#[test]
fn test_layers_bind_rename_delete() {
    let mut editor = editor();
    let before = editor.draft.clone();
    editor.profile().layer.entry("nav".to_string()).or_default();
    draft::set_binding(
        editor.profile(),
        None,
        Group::Buttons,
        "lb",
        Some("layer:hold:nav".into()),
    );
    draft::set_binding(
        editor.profile(),
        Some("nav"),
        Group::Axes,
        "rpad",
        Some("scroll".into()),
    );
    assert_eq!(editor.layer_names(), ["nav"]);
    let mut slots = draft::slots(None);
    draft::set_slot(&mut slots, "press", Some("mouse:left".to_string()));
    draft::set_slot(&mut slots, "long", Some("layer:toggle:nav".to_string()));
    draft::set_binding(
        editor.profile(),
        None,
        Group::Buttons,
        "x",
        draft::pack(slots),
    );
    // Names are taken literally, regex replacement patterns and JSON quotes too
    let odd = "$1\"\\";
    let layer = editor.profile().layer.remove("nav").unwrap();
    editor.profile().layer.insert(odd.to_string(), layer);
    draft::retarget(editor.profile(), "nav", odd);
    assert_eq!(
        editor.profile().buttons["lb"],
        BindingToml::Target(format!("layer:hold:{odd}"))
    );
    assert_eq!(editor.profile().layer[odd].axes["rpad"], "scroll");
    // Bindings that targeted it, in the profile, its layers and its chords, are cleared
    editor.profile().chord[0]
        .buttons
        .insert("a".to_string(), format!("layer:hold:{odd}").into());
    editor.profile().layer.remove(odd);
    draft::untarget(editor.profile(), odd);
    assert!(!editor.profile().buttons.contains_key("lb"));
    assert_eq!(
        editor.profile().buttons["x"],
        BindingToml::Target("mouse:left".to_string())
    );
    assert_eq!(
        editor.profile().chord[0].buttons["a"],
        BindingToml::Target("none".to_string())
    );
    assert!(editor.profile().layer.is_empty());
    assert_eq!(
        changed(&before, &editor.draft),
        ["profile.desktop.buttons.x", "profile.desktop.chord"]
    );
}

#[test]
fn test_config_event_keeps_unsaved_draft() {
    let mut editor = editor();
    draft::set_binding(
        editor.profile(),
        None,
        Group::Buttons,
        "a",
        Some("mouse:left".into()),
    );
    editor.set_file(config(), false);
    assert!(editor.dirty(), "an unsaved draft survives a Config event");
    editor.set_file(config(), true);
    assert!(!editor.dirty(), "revert replaces it");
}

#[test]
fn test_profiles() {
    let mut editor = editor();
    assert_eq!(
        draft::profile_names(&editor.draft),
        ["browser", "desktop", "media", "pad"]
    );
    assert!(!editor.deletable(), "the baseline defines desktop");
    editor.new_profile("desktop");
    assert_eq!(editor.error, "Profile desktop exists");
    editor.new_profile("game");
    assert_eq!(editor.editing(), "game");
    assert_eq!(
        editor.draft.profile["game"],
        editor.draft.profile["desktop"]
    );
    assert!(editor.deletable(), "only the editor's drop-in defines game");
    editor.delete_profile();
    assert_eq!(editor.editing(), "pad");
    assert!(!editor.draft.profile.contains_key("game"));
}

#[test]
fn test_error_placement() {
    let placed = draft::place("Profile browser: buttons.a: no such key");
    assert_eq!(placed.profile.as_deref(), Some("browser"));
    assert_eq!(placed.path.as_deref(), Some("buttons.a"));
    assert_eq!(placed.text, "no such key");
    let placed = draft::place("auto.rule.1.app_id: bad regex");
    assert_eq!(placed.path.as_deref(), Some("auto.rule.1"));
    assert_eq!(placed.text, "auto.rule.1.app_id: bad regex");
    let placed = draft::place("config is read-only: /etc");
    assert_eq!(placed.path, None);
    // A save the daemon refuses with a message naming a row switches to that profile
    let mut editor = editor();
    editor.show_error("Profile browser: chord.0.dpad_up: why");
    assert_eq!(editor.editing(), "browser");
    assert_eq!(editor.row_errors["chord.0.dpad_up"], "why");
    editor.set_file(config(), true);
    assert!(editor.row_errors.is_empty(), "a fresh file clears them");
}

#[test]
fn test_exact_app_id() {
    assert_eq!(draft::exact("steam_app_1234"), "^steam_app_1234$");
    assert_eq!(draft::exact("a.b"), "^a\\.b$");
}
