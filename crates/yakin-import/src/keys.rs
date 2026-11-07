//! Steam Input key names (`key_press <NAME>`) and the evdev key each one presses.

pub const KEYS: [(&str, &str); 123] = [
    ("A", "KEY_A"),
    ("B", "KEY_B"),
    ("C", "KEY_C"),
    ("D", "KEY_D"),
    ("E", "KEY_E"),
    ("F", "KEY_F"),
    ("G", "KEY_G"),
    ("H", "KEY_H"),
    ("I", "KEY_I"),
    ("J", "KEY_J"),
    ("K", "KEY_K"),
    ("L", "KEY_L"),
    ("M", "KEY_M"),
    ("N", "KEY_N"),
    ("O", "KEY_O"),
    ("P", "KEY_P"),
    ("Q", "KEY_Q"),
    ("R", "KEY_R"),
    ("S", "KEY_S"),
    ("T", "KEY_T"),
    ("U", "KEY_U"),
    ("V", "KEY_V"),
    ("W", "KEY_W"),
    ("X", "KEY_X"),
    ("Y", "KEY_Y"),
    ("Z", "KEY_Z"),
    ("0", "KEY_0"),
    ("1", "KEY_1"),
    ("2", "KEY_2"),
    ("3", "KEY_3"),
    ("4", "KEY_4"),
    ("5", "KEY_5"),
    ("6", "KEY_6"),
    ("7", "KEY_7"),
    ("8", "KEY_8"),
    ("9", "KEY_9"),
    ("F1", "KEY_F1"),
    ("F2", "KEY_F2"),
    ("F3", "KEY_F3"),
    ("F4", "KEY_F4"),
    ("F5", "KEY_F5"),
    ("F6", "KEY_F6"),
    ("F7", "KEY_F7"),
    ("F8", "KEY_F8"),
    ("F9", "KEY_F9"),
    ("F10", "KEY_F10"),
    ("F11", "KEY_F11"),
    ("F12", "KEY_F12"),
    ("F13", "KEY_F13"),
    ("F14", "KEY_F14"),
    ("F15", "KEY_F15"),
    ("F16", "KEY_F16"),
    ("F17", "KEY_F17"),
    ("F18", "KEY_F18"),
    ("F19", "KEY_F19"),
    ("F20", "KEY_F20"),
    ("F21", "KEY_F21"),
    ("F22", "KEY_F22"),
    ("F23", "KEY_F23"),
    ("F24", "KEY_F24"),
    ("KEYPAD_0", "KEY_KP0"),
    ("KEYPAD_1", "KEY_KP1"),
    ("KEYPAD_2", "KEY_KP2"),
    ("KEYPAD_3", "KEY_KP3"),
    ("KEYPAD_4", "KEY_KP4"),
    ("KEYPAD_5", "KEY_KP5"),
    ("KEYPAD_6", "KEY_KP6"),
    ("KEYPAD_7", "KEY_KP7"),
    ("KEYPAD_8", "KEY_KP8"),
    ("KEYPAD_9", "KEY_KP9"),
    ("ESCAPE", "KEY_ESC"),
    ("BACKTICK", "KEY_GRAVE"),
    ("DASH", "KEY_MINUS"),
    ("EQUALS", "KEY_EQUAL"),
    ("BACKSPACE", "KEY_BACKSPACE"),
    ("TAB", "KEY_TAB"),
    ("LEFT_BRACKET", "KEY_LEFTBRACE"),
    ("RIGHT_BRACKET", "KEY_RIGHTBRACE"),
    ("BACKSLASH", "KEY_BACKSLASH"),
    ("CAPSLOCK", "KEY_CAPSLOCK"),
    ("SEMICOLON", "KEY_SEMICOLON"),
    ("SINGLE_QUOTE", "KEY_APOSTROPHE"),
    ("ENTER", "KEY_ENTER"),
    ("RETURN", "KEY_ENTER"),
    ("COMMA", "KEY_COMMA"),
    ("PERIOD", "KEY_DOT"),
    ("FORWARD_SLASH", "KEY_SLASH"),
    ("SPACE", "KEY_SPACE"),
    ("LEFT_SHIFT", "KEY_LEFTSHIFT"),
    ("RIGHT_SHIFT", "KEY_RIGHTSHIFT"),
    ("LEFT_CONTROL", "KEY_LEFTCTRL"),
    ("RIGHT_CONTROL", "KEY_RIGHTCTRL"),
    ("LEFT_ALT", "KEY_LEFTALT"),
    ("RIGHT_ALT", "KEY_RIGHTALT"),
    ("LEFT_WINDOWS", "KEY_LEFTMETA"),
    ("RIGHT_WINDOWS", "KEY_RIGHTMETA"),
    ("PRINTSCREEN", "KEY_SYSRQ"),
    ("SCROLL_LOCK", "KEY_SCROLLLOCK"),
    ("PAUSE", "KEY_PAUSE"),
    ("INSERT", "KEY_INSERT"),
    ("DELETE", "KEY_DELETE"),
    ("HOME", "KEY_HOME"),
    ("END", "KEY_END"),
    ("PAGE_UP", "KEY_PAGEUP"),
    ("PAGE_DOWN", "KEY_PAGEDOWN"),
    ("UP_ARROW", "KEY_UP"),
    ("DOWN_ARROW", "KEY_DOWN"),
    ("LEFT_ARROW", "KEY_LEFT"),
    ("RIGHT_ARROW", "KEY_RIGHT"),
    ("NUM_LOCK", "KEY_NUMLOCK"),
    ("KEYPAD_DIVIDE", "KEY_KPSLASH"),
    ("KEYPAD_MULTIPLY", "KEY_KPASTERISK"),
    ("KEYPAD_DASH", "KEY_KPMINUS"),
    ("KEYPAD_PLUS", "KEY_KPPLUS"),
    ("KEYPAD_ENTER", "KEY_KPENTER"),
    ("KEYPAD_PERIOD", "KEY_KPDOT"),
    ("VOLUME_UP", "KEY_VOLUMEUP"),
    ("VOLUME_DOWN", "KEY_VOLUMEDOWN"),
    ("MUTE", "KEY_MUTE"),
    ("PLAY", "KEY_PLAYPAUSE"),
    ("NEXT_TRACK", "KEY_NEXTSONG"),
    ("PREV_TRACK", "KEY_PREVIOUSSONG"),
    ("STOP", "KEY_STOPCD"),
];

/// The `KEY_*` name for a Steam key name, any case.
pub fn key(steam: &str) -> Option<&'static str> {
    KEYS.iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(steam))
        .map(|(_, key)| *key)
}

#[cfg(test)]
mod tests {
    use yakin_core::profile::Target;

    use super::*;

    #[test]
    fn test_keys_are_evdev_keys() {
        for (steam, key) in KEYS {
            let target = format!("key:{key}");
            assert!(
                target.parse::<Target>().is_ok(),
                "{steam} maps to unknown {key}"
            );
        }
    }

    #[test]
    fn test_key_any_case() {
        assert_eq!(key("up_arrow"), Some("KEY_UP"));
        assert_eq!(key("RETURN"), Some("KEY_ENTER"));
        assert_eq!(key("NOPE"), None);
    }
}
