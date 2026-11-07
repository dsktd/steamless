//! The on-screen keyboard: a QWERTY layout split between the two trackpads, typed through the
//! daemon's keys device while it is open, and drawn by the overlay.

use evdev::KeyCode;

use crate::menu::Direction;
use crate::menu::MenuCommand;
use crate::output::Output;
use crate::profile::TICK_MS;
use crate::state::Button;
use crate::state::ControllerState;
use crate::state::Side;
use crate::state::Trackpad;

/// Columns of every row; each trackpad points at half of them.
pub const COLUMNS: usize = 10;

/// Columns under one trackpad.
const HALF: usize = COLUMNS / 2;

pub const ROWS: usize = 5;

/// Share of travel past which a trigger types.
const TRIGGER_PULL: f32 = 0.5;

/// The bottom row of both layers, ten columns wide.
const BOTTOM: [(Key, usize); 6] = [
    (Key::Shift, 2),
    (Key::Layer, 1),
    (Key::Space, 3),
    (Key::Backspace, 2),
    (Key::Enter, 1),
    (Key::Close, 1),
];

const LETTERS: [&[(Key, usize)]; ROWS] = [
    &chars([
        ('1', '!'),
        ('2', '@'),
        ('3', '#'),
        ('4', '$'),
        ('5', '%'),
        ('6', '^'),
        ('7', '&'),
        ('8', '*'),
        ('9', '('),
        ('0', ')'),
    ]),
    &chars([
        ('q', 'Q'),
        ('w', 'W'),
        ('e', 'E'),
        ('r', 'R'),
        ('t', 'T'),
        ('y', 'Y'),
        ('u', 'U'),
        ('i', 'I'),
        ('o', 'O'),
        ('p', 'P'),
    ]),
    &chars([
        ('a', 'A'),
        ('s', 'S'),
        ('d', 'D'),
        ('f', 'F'),
        ('g', 'G'),
        ('h', 'H'),
        ('j', 'J'),
        ('k', 'K'),
        ('l', 'L'),
        ('\'', '"'),
    ]),
    &chars([
        ('z', 'Z'),
        ('x', 'X'),
        ('c', 'C'),
        ('v', 'V'),
        ('b', 'B'),
        ('n', 'N'),
        ('m', 'M'),
        (',', '<'),
        ('.', '>'),
        ('/', '?'),
    ]),
    &BOTTOM,
];

const SYMBOLS: [&[(Key, usize)]; ROWS] = [
    &same("!@#$%^&*()"),
    &same("`~-_=+[]{}"),
    &same("\\|;:'\"<>?/"),
    &[
        (Key::Code("Esc", KeyCode::KEY_ESC.0), 1),
        (Key::Code("Tab", KeyCode::KEY_TAB.0), 1),
        (Key::Code("Home", KeyCode::KEY_HOME.0), 1),
        (Key::Code("End", KeyCode::KEY_END.0), 1),
        (Key::Code("PgUp", KeyCode::KEY_PAGEUP.0), 1),
        (Key::Code("PgDn", KeyCode::KEY_PAGEDOWN.0), 1),
        (Key::Code("←", KeyCode::KEY_LEFT.0), 1),
        (Key::Code("↓", KeyCode::KEY_DOWN.0), 1),
        (Key::Code("↑", KeyCode::KEY_UP.0), 1),
        (Key::Code("→", KeyCode::KEY_RIGHT.0), 1),
    ],
    &BOTTOM,
];

/// US layout: a character, what shift makes of it, and its key.
const US: [(char, char, KeyCode); 47] = [
    ('1', '!', KeyCode::KEY_1),
    ('2', '@', KeyCode::KEY_2),
    ('3', '#', KeyCode::KEY_3),
    ('4', '$', KeyCode::KEY_4),
    ('5', '%', KeyCode::KEY_5),
    ('6', '^', KeyCode::KEY_6),
    ('7', '&', KeyCode::KEY_7),
    ('8', '*', KeyCode::KEY_8),
    ('9', '(', KeyCode::KEY_9),
    ('0', ')', KeyCode::KEY_0),
    ('-', '_', KeyCode::KEY_MINUS),
    ('=', '+', KeyCode::KEY_EQUAL),
    ('[', '{', KeyCode::KEY_LEFTBRACE),
    (']', '}', KeyCode::KEY_RIGHTBRACE),
    ('\\', '|', KeyCode::KEY_BACKSLASH),
    (';', ':', KeyCode::KEY_SEMICOLON),
    ('\'', '"', KeyCode::KEY_APOSTROPHE),
    ('`', '~', KeyCode::KEY_GRAVE),
    (',', '<', KeyCode::KEY_COMMA),
    ('.', '>', KeyCode::KEY_DOT),
    ('/', '?', KeyCode::KEY_SLASH),
    ('a', 'A', KeyCode::KEY_A),
    ('b', 'B', KeyCode::KEY_B),
    ('c', 'C', KeyCode::KEY_C),
    ('d', 'D', KeyCode::KEY_D),
    ('e', 'E', KeyCode::KEY_E),
    ('f', 'F', KeyCode::KEY_F),
    ('g', 'G', KeyCode::KEY_G),
    ('h', 'H', KeyCode::KEY_H),
    ('i', 'I', KeyCode::KEY_I),
    ('j', 'J', KeyCode::KEY_J),
    ('k', 'K', KeyCode::KEY_K),
    ('l', 'L', KeyCode::KEY_L),
    ('m', 'M', KeyCode::KEY_M),
    ('n', 'N', KeyCode::KEY_N),
    ('o', 'O', KeyCode::KEY_O),
    ('p', 'P', KeyCode::KEY_P),
    ('q', 'Q', KeyCode::KEY_Q),
    ('r', 'R', KeyCode::KEY_R),
    ('s', 'S', KeyCode::KEY_S),
    ('t', 'T', KeyCode::KEY_T),
    ('u', 'U', KeyCode::KEY_U),
    ('v', 'V', KeyCode::KEY_V),
    ('w', 'W', KeyCode::KEY_W),
    ('x', 'X', KeyCode::KEY_X),
    ('y', 'Y', KeyCode::KEY_Y),
    ('z', 'Z', KeyCode::KEY_Z),
];

/// Keys typed besides the characters and [`Key::Code`]s.
const EXTRA_KEYS: [KeyCode; 6] = [
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_SPACE,
    KeyCode::KEY_BACKSPACE,
    KeyCode::KEY_ENTER,
    KeyCode::KEY_LEFT,
    KeyCode::KEY_RIGHT,
];

/// A row of characters with their shifted forms.
const fn chars(pairs: [(char, char); COLUMNS]) -> [(Key, usize); COLUMNS] {
    let mut row = [(Key::Space, 1); COLUMNS];
    let mut index = 0;
    while index < COLUMNS {
        row[index] = (Key::Char(pairs[index].0, pairs[index].1), 1);
        index += 1;
    }
    row
}

/// A row of characters shift leaves alone.
const fn same(text: &str) -> [(Key, usize); COLUMNS] {
    let bytes = text.as_bytes();
    let mut row = [(Key::Space, 1); COLUMNS];
    let mut index = 0;
    while index < COLUMNS {
        let char = bytes[index] as char;
        row[index] = (Key::Char(char, char), 1);
        index += 1;
    }
    row
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum Layer {
    #[default]
    Letters,
    /// Digits' symbols and navigation keys: "123".
    Symbols,
}
impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Letters => "abc",
            Layer::Symbols => "123",
        }
    }

    /// Rows of keys and the columns each spans.
    pub fn rows(self) -> [&'static [(Key, usize)]; ROWS] {
        match self {
            Layer::Letters => LETTERS,
            Layer::Symbols => SYMBOLS,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Key {
    /// A character and what shift makes of it.
    Char(char, char),
    /// A key typed as it is, with its label.
    Code(&'static str, u16),
    Shift,
    /// Switches between the layers.
    Layer,
    Space,
    Backspace,
    Enter,
    Close,
}

/// The open keyboard: its layer, modifiers, the key under each finger and the D-pad's focus.
/// Keys are numbered row by row; both layers have the same shape.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Keyboard {
    pub layer: Layer,
    /// One-shot: the next character only.
    pub shift: bool,
    pub caps: bool,
    pub hover_left: Option<usize>,
    pub hover_right: Option<usize>,
    pub focus: usize,
}
impl Keyboard {
    /// Point `side`'s finger at the key under `pad`, or lift it.
    pub fn hover(
        &mut self,
        side: Side,
        pad: Option<(i16, i16)>,
    ) {
        let cell = pad.map(|(x, y)| pad_cell(side, x, y));
        match side {
            Side::Left => self.hover_left = cell,
            Side::Right => self.hover_right = cell,
        }
    }

    /// Type the key under `side`'s finger.
    pub fn press(
        &mut self,
        side: Side,
    ) -> Vec<Output> {
        let hover = match side {
            Side::Left => self.hover_left,
            Side::Right => self.hover_right,
        };
        match hover {
            Some(cell) => self.type_key(self.key(cell)),
            None => Vec::new(),
        }
    }

    /// Move the focus one key, wrapping around each edge; up and down keep the column.
    pub fn nav(
        &mut self,
        direction: Direction,
    ) {
        let rows = self.layer.rows();
        let (row, index) = position(rows, self.focus);
        let width = rows[row].len();
        let first = self.focus - index;
        self.focus = match direction {
            Direction::Left => first + (index + width - 1) % width,
            Direction::Right => first + (index + 1) % width,
            Direction::Up | Direction::Down => {
                let to = match direction {
                    Direction::Up => (row + ROWS - 1) % ROWS,
                    _ => (row + 1) % ROWS,
                };
                cell_at(rows, to, start(rows[row], index))
            }
        };
    }

    /// Type the focused key.
    pub fn activate(&mut self) -> Vec<Output> {
        self.type_key(self.key(self.focus))
    }

    /// The key at `cell` of the current layer.
    pub fn key(
        &self,
        cell: usize,
    ) -> Key {
        self.layer
            .rows()
            .iter()
            .flat_map(|row| row.iter())
            .nth(cell)
            .map_or(Key::Space, |(key, _)| *key)
    }

    /// What `key` reads as under the current modifiers.
    pub fn label(
        &self,
        key: Key,
    ) -> String {
        match key {
            Key::Char(plain, shifted) => match self.shifted(plain) {
                true => shifted.to_string(),
                false => plain.to_string(),
            },
            Key::Code(label, _) => label.to_string(),
            Key::Shift => "Shift".to_string(),
            Key::Layer => match self.layer {
                Layer::Letters => Layer::Symbols.name().to_string(),
                Layer::Symbols => Layer::Letters.name().to_string(),
            },
            Key::Space => "Space".to_string(),
            Key::Backspace => "⌫".to_string(),
            Key::Enter => "Enter".to_string(),
            Key::Close => "✕".to_string(),
        }
    }

    /// The outputs of `key`, changing the modifiers and layer as it asks.
    pub fn type_key(
        &mut self,
        key: Key,
    ) -> Vec<Output> {
        match key {
            Key::Char(plain, shifted) => {
                let char = match self.shifted(plain) {
                    true => shifted,
                    false => plain,
                };
                self.shift = false;
                char_outputs(char)
            }
            Key::Code(_, code) => tap(code),
            // Off, one-shot, caps lock, off
            Key::Shift => {
                (self.shift, self.caps) = match (self.shift, self.caps) {
                    (false, false) => (true, false),
                    (true, _) => (false, true),
                    (false, true) => (false, false),
                };
                Vec::new()
            }
            Key::Layer => {
                self.layer = match self.layer {
                    Layer::Letters => Layer::Symbols,
                    Layer::Symbols => Layer::Letters,
                };
                Vec::new()
            }
            Key::Space => tap(KeyCode::KEY_SPACE.0),
            Key::Backspace => tap(KeyCode::KEY_BACKSPACE.0),
            Key::Enter => tap(KeyCode::KEY_ENTER.0),
            Key::Close => vec![Output::Keyboard(MenuCommand::Close)],
        }
    }

    /// The outputs of one controller report from `prev` to `state`; `close` are the buttons
    /// bound to close the keyboard, besides Steam and B. A finger moving onto another key, the focus
    /// moving and a key typed tick at `haptics` (`haptics.menu`) on their side.
    pub fn read(
        &mut self,
        prev: &ControllerState,
        state: &ControllerState,
        close: &[Button],
        haptics: f32,
    ) -> Vec<Output> {
        let pressed =
            |button: Button| !prev.buttons.contains(button) && state.buttons.contains(button);
        let pull = TRIGGER_PULL * i16::MAX as f32;
        let pulled = |prev: u16, now: u16| (prev as f32) < pull && now as f32 >= pull;
        let finger = |pad: &Trackpad| pad.touched.then_some((pad.x, pad.y));
        let before = (self.hover_left, self.hover_right, self.focus);
        self.hover(Side::Left, finger(&state.lpad));
        self.hover(Side::Right, finger(&state.rpad));
        let mut outputs = Vec::new();
        let tick = |side| Output::haptic(side, haptics, TICK_MS);
        for (side, was, is) in [
            (Side::Left, before.0, self.hover_left),
            (Side::Right, before.1, self.hover_right),
        ] {
            if is.is_some() && was.is_some() && is != was {
                outputs.extend(tick(side));
            }
        }
        for (side, pressed) in [
            (
                Side::Left,
                pressed(Button::LPadClick) || pulled(prev.lt, state.lt),
            ),
            (
                Side::Right,
                pressed(Button::RPadClick) || pulled(prev.rt, state.rt),
            ),
        ] {
            let hover = match side {
                Side::Left => self.hover_left,
                Side::Right => self.hover_right,
            };
            if pressed && hover.is_some() {
                outputs.extend(tick(side));
                outputs.extend(self.press(side));
            }
        }
        for (button, direction) in [
            (Button::DpadUp, Direction::Up),
            (Button::DpadDown, Direction::Down),
            (Button::DpadLeft, Direction::Left),
            (Button::DpadRight, Direction::Right),
        ] {
            if pressed(button) {
                self.nav(direction);
            }
        }
        if self.focus != before.2 {
            outputs.extend(tick(Side::Left));
        }
        if pressed(Button::A) {
            outputs.extend(tick(Side::Right));
            outputs.extend(self.activate());
        }
        for (button, key) in [
            (Button::View, Key::Backspace),
            (Button::Y, Key::Space),
            (Button::X, Key::Shift),
            (Button::Menu, Key::Enter),
            (Button::Lb, Key::Code("←", KeyCode::KEY_LEFT.0)),
            (Button::Rb, Key::Code("→", KeyCode::KEY_RIGHT.0)),
        ] {
            if pressed(button) {
                outputs.extend(self.type_key(key));
            }
        }
        // B backs out of the keyboard like Esc everywhere else
        if pressed(Button::Steam)
            || pressed(Button::B)
            || close.iter().any(|button| pressed(*button))
        {
            outputs.push(Output::Keyboard(MenuCommand::Close));
        }
        outputs
    }

    /// Whether `plain` types its shifted form now: shift does it for all, caps for letters.
    fn shifted(
        &self,
        plain: char,
    ) -> bool {
        self.shift || (self.caps && plain.is_ascii_alphabetic())
    }
}

/// Every key code the keyboard can type, for the keys device.
pub fn keys() -> Vec<u16> {
    let codes = [Layer::Letters, Layer::Symbols]
        .into_iter()
        .flat_map(Layer::rows)
        .flat_map(|row| row.iter())
        .filter_map(|(key, _)| match key {
            Key::Code(_, code) => Some(*code),
            _ => None,
        });
    US.iter()
        .map(|(_, _, code)| code.0)
        .chain(EXTRA_KEYS.iter().map(|code| code.0))
        .chain(codes)
        .collect()
}

/// The key a finger at `(x, y)` on `side`'s trackpad points at: the pad spans its half of the
/// columns and every row, Y up.
fn pad_cell(
    side: Side,
    x: i16,
    y: i16,
) -> usize {
    let share = |value: f32| (value / u16::MAX as f32).clamp(0.0, 1.0);
    let column = ((share(x as f32 - i16::MIN as f32) * HALF as f32) as usize).min(HALF - 1);
    let row = ((share(i16::MAX as f32 - y as f32) * ROWS as f32) as usize).min(ROWS - 1);
    let column = match side {
        Side::Left => column,
        Side::Right => column + HALF,
    };
    cell_at(Layer::Letters.rows(), row, column)
}

/// The key of `row` that spans `column`.
fn cell_at(
    rows: [&[(Key, usize)]; ROWS],
    row: usize,
    column: usize,
) -> usize {
    let before: usize = rows[..row].iter().map(|row| row.len()).sum();
    let mut end = 0;
    for (index, (_, span)) in rows[row].iter().enumerate() {
        end += span;
        if column < end {
            return before + index;
        }
    }
    before + rows[row].len() - 1
}

/// Row of `cell` and its index within the row.
fn position(
    rows: [&[(Key, usize)]; ROWS],
    cell: usize,
) -> (usize, usize) {
    let mut cell = cell;
    for (index, row) in rows.iter().enumerate() {
        if cell < row.len() {
            return (index, cell);
        }
        cell -= row.len();
    }
    (0, 0)
}

/// The first column of key `index` in `row`.
fn start(
    row: &[(Key, usize)],
    index: usize,
) -> usize {
    row[..index].iter().map(|(_, span)| span).sum()
}

fn tap(code: u16) -> Vec<Output> {
    vec![Output::Key(code, true), Output::Key(code, false)]
}

/// A press and release of the key that types `char`, inside left shift where it needs it.
fn char_outputs(char: char) -> Vec<Output> {
    let Some((plain, _, code)) = US
        .iter()
        .find(|(plain, shifted, _)| *plain == char || *shifted == char)
    else {
        return Vec::new();
    };
    let shift = KeyCode::KEY_LEFTSHIFT.0;
    match *plain == char {
        true => tap(code.0),
        false => [
            vec![Output::Key(shift, true)],
            tap(code.0),
            vec![Output::Key(shift, false)],
        ]
        .concat(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Buttons;

    const MAX: i16 = i16::MAX;
    const MIN: i16 = i16::MIN;

    #[test]
    fn test_pad_cells_cover_their_half() {
        // Top left, top right of each pad
        assert_eq!(pad_cell(Side::Left, MIN, MAX), 0);
        assert_eq!(pad_cell(Side::Left, MAX, MAX), 4);
        assert_eq!(pad_cell(Side::Right, MIN, MAX), 5);
        assert_eq!(pad_cell(Side::Right, MAX, MAX), 9);
        // The centre of the left pad: row 2 of 5, column 2 of 5
        assert_eq!(pad_cell(Side::Left, 0, 0), 22);
        // Bottom row: shift spans columns 0-1, space 3-5, backspace 6-7
        let bottom = 40;
        assert_eq!(pad_cell(Side::Left, MIN, MIN), bottom);
        assert_eq!(pad_cell(Side::Left, -10000, MIN), bottom);
        assert_eq!(pad_cell(Side::Left, MAX, MIN), bottom + 2);
        assert_eq!(pad_cell(Side::Right, MIN, MIN), bottom + 2);
        assert_eq!(pad_cell(Side::Right, MAX, MIN), bottom + 5);
    }

    #[test]
    fn test_hover_and_press() {
        let mut keyboard = Keyboard::default();
        assert!(keyboard.press(Side::Left).is_empty());
        keyboard.hover(Side::Right, Some((MIN, MAX)));
        assert_eq!(keyboard.hover_right, Some(5));
        assert_eq!(keyboard.press(Side::Right), tap(KeyCode::KEY_6.0));
        keyboard.hover(Side::Right, None);
        assert_eq!(keyboard.hover_right, None);
    }

    #[test]
    fn test_shift_is_one_shot_and_twice_is_caps() {
        let mut keyboard = Keyboard::default();
        let q = keyboard.key(10);
        let shift = KeyCode::KEY_LEFTSHIFT.0;
        let shifted_q = vec![
            Output::Key(shift, true),
            Output::Key(KeyCode::KEY_Q.0, true),
            Output::Key(KeyCode::KEY_Q.0, false),
            Output::Key(shift, false),
        ];
        keyboard.type_key(Key::Shift);
        assert_eq!(keyboard.label(q), "Q");
        assert_eq!(keyboard.type_key(q), shifted_q);
        assert_eq!(keyboard.type_key(q), tap(KeyCode::KEY_Q.0));
        keyboard.type_key(Key::Shift);
        keyboard.type_key(Key::Shift);
        assert!(keyboard.caps && !keyboard.shift);
        assert_eq!(keyboard.type_key(q), shifted_q);
        assert_eq!(keyboard.type_key(q), shifted_q);
        // Caps leaves digits alone
        assert_eq!(keyboard.type_key(keyboard.key(0)), tap(KeyCode::KEY_1.0));
        keyboard.type_key(Key::Shift);
        assert!(!keyboard.caps && !keyboard.shift);
    }

    #[test]
    fn test_layer_switch() {
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.label(Key::Layer), "123");
        keyboard.type_key(Key::Layer);
        assert_eq!(keyboard.layer, Layer::Symbols);
        assert_eq!(keyboard.label(Key::Layer), "abc");
        assert_eq!(keyboard.key(0), Key::Char('!', '!'));
        assert_eq!(
            keyboard.type_key(keyboard.key(0)),
            [
                vec![Output::Key(KeyCode::KEY_LEFTSHIFT.0, true)],
                tap(KeyCode::KEY_1.0),
                vec![Output::Key(KeyCode::KEY_LEFTSHIFT.0, false)],
            ]
            .concat()
        );
        assert_eq!(
            keyboard.type_key(keyboard.key(36)),
            tap(KeyCode::KEY_LEFT.0)
        );
        keyboard.type_key(Key::Layer);
        assert_eq!(keyboard.layer, Layer::Letters);
    }

    #[test]
    fn test_layers_share_a_shape() {
        for (letters, symbols) in LETTERS.iter().zip(SYMBOLS) {
            assert_eq!(letters.len(), symbols.len());
            let span = |row: &[(Key, usize)]| row.iter().map(|(_, span)| span).sum::<usize>();
            assert_eq!(span(letters), COLUMNS);
            assert_eq!(span(symbols), COLUMNS);
        }
    }

    #[test]
    fn test_every_typed_key_is_advertised() {
        let keys = keys();
        let mut keyboard = Keyboard::default();
        for layer in [Layer::Letters, Layer::Symbols] {
            keyboard.layer = layer;
            for cell in 0..46 {
                for shift in [false, true] {
                    keyboard.shift = shift;
                    let key = keyboard.key(cell);
                    let typed = keyboard.clone().type_key(key);
                    if let Key::Char(..) = key {
                        assert!(!typed.is_empty(), "{key:?}");
                    }
                    for output in typed {
                        if let Output::Key(code, _) = output {
                            assert!(keys.contains(&code), "{key:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_nav_wraps_and_keeps_the_column() {
        let mut keyboard = Keyboard::default();
        keyboard.nav(Direction::Left);
        assert_eq!(keyboard.focus, 9);
        keyboard.nav(Direction::Up);
        // Column 9 of the bottom row: close
        assert_eq!(keyboard.key(keyboard.focus), Key::Close);
        keyboard.nav(Direction::Left);
        keyboard.nav(Direction::Left);
        assert_eq!(keyboard.key(keyboard.focus), Key::Backspace);
        // From backspace (columns 6-7) up to column 6
        keyboard.nav(Direction::Up);
        assert_eq!(keyboard.focus, 36);
        keyboard.nav(Direction::Down);
        keyboard.nav(Direction::Down);
        assert_eq!(keyboard.focus, 6);
    }

    #[test]
    fn test_read_routes_the_controller() {
        let mut keyboard = Keyboard::default();
        let idle = ControllerState::default();
        let mut touch = idle;
        touch.lpad.touched = true;
        touch.lpad.x = MIN;
        touch.lpad.y = MAX;
        assert!(keyboard.read(&idle, &touch, &[], 0.0).is_empty());
        assert_eq!(keyboard.hover_left, Some(0));
        let mut click = touch;
        click.buttons = Buttons::empty().with(Button::LPadClick);
        assert_eq!(
            keyboard.read(&touch, &click, &[], 0.0),
            tap(KeyCode::KEY_1.0)
        );
        let mut trigger = touch;
        trigger.lt = u16::MAX;
        assert_eq!(
            keyboard.read(&touch, &trigger, &[], 0.0),
            tap(KeyCode::KEY_1.0)
        );
        assert_eq!(keyboard.read(&trigger, &idle, &[], 0.0), Vec::new());
        assert_eq!(keyboard.hover_left, None);
        let press = |button: Button| ControllerState {
            buttons: Buttons::empty().with(button),
            ..idle
        };
        let mut read = |button| keyboard.read(&idle, &press(button), &[Button::Quick], 0.0);
        assert_eq!(read(Button::View), tap(KeyCode::KEY_BACKSPACE.0));
        assert_eq!(read(Button::Menu), tap(KeyCode::KEY_ENTER.0));
        assert_eq!(read(Button::Rb), tap(KeyCode::KEY_RIGHT.0));
        let close = vec![Output::Keyboard(MenuCommand::Close)];
        assert_eq!(read(Button::Steam), close);
        assert_eq!(read(Button::B), close);
        assert_eq!(read(Button::Quick), close);
        assert!(read(Button::X).is_empty());
        assert!(keyboard.shift);
    }

    #[test]
    fn test_read_ticks() {
        let mut keyboard = Keyboard::default();
        let idle = ControllerState::default();
        let tick = |side| Output::Haptic {
            side,
            strength: 255,
            ms: TICK_MS,
        };
        let mut touch = idle;
        touch.lpad.touched = true;
        touch.lpad.x = MIN;
        touch.lpad.y = MAX;
        // Landing is silent, moving onto the next key ticks
        assert!(keyboard.read(&idle, &touch, &[], 1.0).is_empty());
        let mut next = touch;
        next.lpad.x = 0;
        assert_eq!(
            keyboard.read(&touch, &next, &[], 1.0),
            vec![tick(Side::Left)]
        );
        assert!(keyboard.read(&next, &next, &[], 1.0).is_empty());
        let mut click = next;
        click.buttons = Buttons::empty().with(Button::LPadClick);
        let outputs = keyboard.read(&next, &click, &[], 1.0);
        assert_eq!(outputs[0], tick(Side::Left));
        assert_eq!(outputs.len(), 3);
        let press = |button: Button| ControllerState {
            buttons: Buttons::empty().with(button),
            ..idle
        };
        assert_eq!(
            keyboard.read(&idle, &press(Button::DpadRight), &[], 1.0),
            vec![tick(Side::Left)]
        );
        assert_eq!(
            keyboard.read(&idle, &press(Button::A), &[], 1.0)[0],
            tick(Side::Right)
        );
        // Off at 0
        assert!(
            keyboard
                .read(&idle, &press(Button::DpadRight), &[], 0.0)
                .is_empty()
        );
    }
}
