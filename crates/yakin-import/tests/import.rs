use yakin_core::profile::ActivatorsToml;
use yakin_core::profile::BindingToml;
use yakin_core::profile::Config;
use yakin_import::import;
use yakin_import::tally::Kind;

const FIXTURE: &str = include_str!("fixture.vdf");

#[test]
fn test_import_buttons() {
    let imported = import(FIXTURE, "fixture").unwrap();
    let buttons = &imported.profiles["fixture-default"].buttons;
    assert_eq!(
        buttons["a"],
        BindingToml::Activators(ActivatorsToml {
            press: Some("pad:south".to_string()),
            double: Some("mouse:middle".to_string()),
            start: Some("key:KEY_F".to_string()),
            release: Some("key:KEY_G".to_string()),
            double_ms: Some(300),
            turbo_ms: Some(100),
            ..ActivatorsToml::default()
        })
    );
    assert_eq!(
        buttons["b"],
        BindingToml::Activators(ActivatorsToml {
            // Two key_press bindings on one activator press together
            press: Some("key:KEY_ESC+KEY_TAB".to_string()),
            long: Some("key:KEY_Q".to_string()),
            long_ms: Some(500),
            ..ActivatorsToml::default()
        })
    );
    assert_eq!(buttons["dpad_up"], "key:KEY_UP");
    assert_eq!(buttons["menu"], "key:KEY_ENTER");
    assert_eq!(buttons["lb"], "mouse:middle");
    assert_eq!(buttons["lstick_click"], "pad:thumbl");
    assert_eq!(buttons["rt_full"], "mouse:right");
    assert_eq!(buttons["rpad_click"], "mouse:left");
    assert!(!buttons.contains_key("x"));
}

#[test]
fn test_import_axes() {
    let imported = import(FIXTURE, "fixture").unwrap();
    let axes = &imported.profiles["fixture-default"].axes;
    assert_eq!(axes["lstick"], "pad:left");
    assert_eq!(axes["rstick"], "keys:KEY_W,KEY_S,KEY_A,KEY_D");
    assert_eq!(axes["rt"], "pad:rt");
    assert_eq!(axes["rpad"], "cursor");
    assert_eq!(axes["lpad"], "scroll");
    assert_eq!(imported.profiles["fixture-default"].mouse.speed, Some(1.5));
}

#[test]
fn test_import_stick_mouse_and_flick() {
    let vdf = FIXTURE
        .replace("\"joystick_move\"", "\"joystick_mouse\"")
        .replace("\"deadzone\"\t\t\"2000\"", "\"sensitivity\"\t\t\"200\"")
        .replace(
            "\"3\"\t\t\"right_joystick active\"",
            "\"8\"\t\t\"right_joystick active\"",
        );
    let imported = import(&vdf, "fixture").unwrap();
    let axes = &imported.profiles["fixture-default"].axes;
    assert_eq!(axes["lstick"], "cursor");
    assert_eq!(axes["rstick"], "flick");
    assert_eq!(
        imported.profiles["fixture-default"].mouse.stick_speed,
        Some(2000.0)
    );
    assert_eq!(imported.profiles["fixture-default"].mouse.speed, Some(1.5));
    assert_eq!(imported.tally.get(Kind::Mode, "joystick_mouse"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Mode, "flickstick")[0], 2);
}

#[test]
fn test_import_mode_shift_chord() {
    let imported = import(FIXTURE, "fixture").unwrap();
    let [chord] = imported.profiles["fixture-default"].chord.as_slice() else {
        panic!("{:?}", imported.profiles["fixture-default"].chord);
    };
    assert_eq!(chord.hold, "y");
    assert_eq!(chord.buttons["a"], "key:KEY_VOLUMEUP");
}

#[test]
fn test_import_skips_are_listed() {
    let imported = import(FIXTURE, "fixture").unwrap();
    let skipped = imported.skipped.join("\n");
    assert!(!skipped.contains("Long_Press"), "{skipped}");
    assert!(
        skipped.contains("button_a Soft_Press: key_press H (analog threshold)"),
        "{skipped}"
    );
    assert!(skipped.contains("game_action"), "{skipped}");
    assert_eq!(imported.tally.get(Kind::Activator, "long_press"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Activator, "double_press"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Activator, "start_press"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Activator, "release"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Activator, "soft_press"), [0, 1]);
    assert_eq!(imported.tally.get(Kind::Activator, "full_press"), [17, 0]);
    assert_eq!(imported.tally.get(Kind::Setting, "long_press_time"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Setting, "repeat_rate"), [1, 0]);
    assert_eq!(imported.tally.get(Kind::Setting, "deadzone"), [0, 1]);
}

#[test]
fn test_import_haptic_intensity() {
    assert_eq!(
        import(FIXTURE, "fixture").unwrap().profiles["fixture-default"]
            .haptics
            .click,
        None
    );
    let intensity = |level: &str| {
        let setting = format!("\"haptic_intensity\"\t\t\"{level}\"\n\t\t\t\t\t\t\t");
        let vdf = FIXTURE.replacen("\"hold_repeats\"", &format!("{setting}\"hold_repeats\""), 1);
        import(&vdf, "fixture").unwrap()
    };
    assert_eq!(
        intensity("0").profiles["fixture-default"].haptics.click,
        Some(0.0)
    );
    assert_eq!(
        intensity("1").profiles["fixture-default"].haptics.click,
        Some(0.2)
    );
    assert_eq!(
        intensity("2").profiles["fixture-default"].haptics.click,
        Some(0.4)
    );
    assert_eq!(
        intensity("3").profiles["fixture-default"].haptics.click,
        Some(0.7)
    );
    let imported = intensity("2");
    assert_eq!(
        imported.tally.get(Kind::Setting, "haptic_intensity"),
        [1, 0]
    );
    Config::parse(&imported.toml().unwrap()).unwrap();
    let unknown = intensity("9");
    assert_eq!(unknown.profiles["fixture-default"].haptics.click, None);
    assert_eq!(unknown.tally.get(Kind::Setting, "haptic_intensity"), [0, 1]);
}

#[test]
fn test_import_toml_loads() {
    let toml = import(FIXTURE, "fixture").unwrap().toml().unwrap();
    assert!(toml.starts_with("# skipped: "));
    Config::parse(&toml).unwrap();
}

#[test]
fn test_import_rejects_other_roots() {
    assert!(import("\"other\" { }", "fixture").is_err());
}

/// The parser keeps repeated keys in order, unescapes quotes, drops `//` comments and returns
/// `#base` lines separately. The corpus has no `#base` lines; Steam supports them in general.
#[test]
fn test_parser() {
    let text = "#base \"other.vdf\"\n\"root\"\n{\n\t// a comment\n\t\"group\" \"one\"\n\t\"x\" \"a \\\"b\\\"\"\n\t\"group\" \"two\" // trailing\n}\n";
    let vdf = keyvalues_parser::parse(text).unwrap();
    assert_eq!(vdf.bases, ["other.vdf"]);
    let root = vdf.value.get_obj().unwrap();
    let groups: Vec<_> = root["group"]
        .iter()
        .filter_map(|value| value.get_str())
        .collect();
    assert_eq!(groups, ["one", "two"]);
    assert_eq!(root["x"][0].get_str(), Some("a \"b\""));
}

#[test]
fn test_import_action_sets() {
    let imported = import(FIXTURE, "fixture").unwrap();
    let names: Vec<&str> = imported.profiles.keys().map(String::as_str).collect();
    assert_eq!(names, ["fixture-default", "fixture-other"]);
    assert_eq!(imported.profiles["fixture-other"].axes["rstick"], "flick");
    // One set keeps the config's name
    let one = FIXTURE.replace("\"name\"\t\t\"Other\"", "\"name\"\t\t\"Layer\"");
    let one = one.replacen(
        "\t\"preset\"",
        "\t\"action_layers\"\n\t{\n\t\t\"Layer\"\n\t\t{\n\t\t\t\"parent_set_name\"\t\t\"Default\"\n\t\t}\n\t}\n\t\"preset\"",
        1,
    );
    let imported = import(&one, "fixture").unwrap();
    let names: Vec<&str> = imported.profiles.keys().map(String::as_str).collect();
    assert_eq!(names, ["fixture"]);
    let layer = &imported.profiles["fixture"].layer["layer"];
    assert_eq!(layer.axes["rstick"], "flick");
    assert!(layer.buttons.is_empty());
    Config::parse(&imported.toml().unwrap()).unwrap();
}

#[test]
fn test_import_change_preset_and_layers() {
    let vdf = FIXTURE.replace(
        "key_press VOLUME_UP",
        "controller_action CHANGE_PRESET 2 1 1",
    );
    let imported = import(&vdf, "fixture").unwrap();
    let chord = &imported.profiles["fixture-default"].chord[0];
    assert_eq!(chord.buttons["a"], "profile:fixture-other");
    Config::parse(&imported.toml().unwrap()).unwrap();
    // Layer actions name layers only
    let vdf = FIXTURE.replace("key_press VOLUME_UP", "controller_action hold_layer 2 1 1");
    let imported = import(&vdf, "fixture").unwrap();
    assert!(
        imported.profiles["fixture-default"].chord[0]
            .buttons
            .is_empty()
    );
}

#[test]
fn test_import_gyro() {
    let vdf = FIXTURE.replace(
        "\"8\"\t\t\"right_joystick active\"",
        "\"10\"\t\t\"gyro active\"",
    );
    let vdf = vdf.replacen(
        "\t\"preset\"",
        "\t\"group\"\n\t{\n\t\t\"id\"\t\t\"10\"\n\t\t\"mode\"\t\t\"gyro_to_mouse\"\n\t\t\"settings\"\n\t\t{\n\t\t\t\"sensitivity\"\t\t\"200\"\n\t\t\t\"gyro_button\"\t\t\"12\"\n\t\t}\n\t}\n\t\"preset\"",
        1,
    );
    let imported = import(&vdf, "fixture").unwrap();
    let other = &imported.profiles["fixture-other"];
    assert_eq!(other.axes["gyro"], "cursor");
    assert_eq!(other.gyro.speed, Some(2.0));
    assert_eq!(imported.tally.get(Kind::Setting, "gyro_button"), [0, 1]);
    Config::parse(&imported.toml().unwrap()).unwrap();
}
