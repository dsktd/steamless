//! What the input looks like on the model, as plain functions of the controller state.

use bevy::prelude::*;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;
use yakin_core::state::Side;
use yakin_core::state::Stick;
use yakin_core::state::Trackpad;

/// A fully deflected stick leans this far, radians.
pub const STICK_LEAN: f32 = 0.35;

/// How a touched but unpressed control glows, against 1 for pressed.
pub const TOUCHED: f32 = 0.35;

/// How brightly a control glows, 0 to 1: 1 pressed, [`TOUCHED`] while a finger rests on it,
/// and a trigger's travel.
pub fn glow(
    state: &ControllerState,
    button: Button,
) -> f32 {
    if state.buttons.contains(button) {
        return 1.0;
    }
    match button {
        Button::LtFull => travel(state.lt),
        Button::RtFull => travel(state.rt),
        _ => match touch(button) {
            Some(touch) if state.buttons.contains(touch) => TOUCHED,
            _ => 0.0,
        },
    }
}

/// The touch sensor under a clickable control.
fn touch(button: Button) -> Option<Button> {
    match button {
        Button::LStickClick => Some(Button::LStickTouch),
        Button::RStickClick => Some(Button::RStickTouch),
        Button::LPadClick => Some(Button::LPadTouch),
        Button::RPadClick => Some(Button::RPadTouch),
        Button::GripL1 | Button::GripL2 => Some(Button::GripLTouch),
        Button::GripR1 | Button::GripR2 => Some(Button::GripRTouch),
        _ => None,
    }
}

/// A trigger's pull, 0 to 1, read as the daemon does.
pub fn travel(value: u16) -> f32 {
    value.min(i16::MAX as u16) as f32 / i16::MAX as f32
}

/// The cap's rotation for a stick: it leans the way it is pushed, up toward the triggers.
pub fn lean(stick: Stick) -> Quat {
    let (x, y) = (unit(stick.x), unit(stick.y));
    Quat::from_rotation_y(x * STICK_LEAN) * Quat::from_rotation_x(-y * STICK_LEAN)
}

/// Where a finger rests on a trackpad, -1 to 1 each way with up positive, while it touches.
pub fn dot(pad: Trackpad) -> Option<Vec2> {
    if !pad.touched {
        return None;
    }
    Some(Vec2::new(unit(pad.x), unit(pad.y)))
}

pub fn stick(
    state: &ControllerState,
    side: Side,
) -> Stick {
    match side {
        Side::Left => state.lstick,
        Side::Right => state.rstick,
    }
}

pub fn pad(
    state: &ControllerState,
    side: Side,
) -> Trackpad {
    match side {
        Side::Left => state.lpad,
        Side::Right => state.rpad,
    }
}

/// An overlay's colour and glow: cyan as it glows, amber while selected, else grey at `rest`
/// opacity (0 hides it).
pub fn tint(
    glow: f32,
    selected: bool,
    rest: f32,
) -> (Color, LinearRgba) {
    const PRESSED: Srgba = Srgba::rgb(0.2, 0.8, 1.0);
    const SELECTED: Srgba = Srgba::rgb(1.0, 0.65, 0.15);
    const REST: Srgba = Srgba::rgb(0.55, 0.55, 0.6);
    let (color, alpha, strength) = match (glow > 0.0, selected) {
        (true, _) => (PRESSED, rest.max(0.3 + 0.6 * glow), 4.0 * glow),
        (false, true) => (SELECTED, rest.max(0.6), 1.5),
        (false, false) => (REST, rest, 0.0),
    };
    (
        color.with_alpha(alpha).into(),
        (LinearRgba::from(color) * strength).with_alpha(1.0),
    )
}

fn unit(value: i16) -> f32 {
    (value as f32 / i16::MAX as f32).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod test {
    use super::*;

    fn pressing(buttons: &[Button]) -> ControllerState {
        let bits = buttons.iter().fold(0, |bits, button| bits | button.mask());
        ControllerState {
            buttons: bits.into(),
            ..Default::default()
        }
    }

    #[test]
    fn test_glow_pressed() {
        let state = pressing(&[Button::A, Button::Lb]);
        assert_eq!(glow(&state, Button::A), 1.0);
        assert_eq!(glow(&state, Button::Lb), 1.0);
        assert_eq!(glow(&state, Button::B), 0.0);
    }

    #[test]
    fn test_glow_touched() {
        let state = pressing(&[Button::LPadTouch, Button::RStickTouch, Button::RStickClick]);
        assert_eq!(glow(&state, Button::LPadClick), TOUCHED);
        assert_eq!(glow(&state, Button::RPadClick), 0.0);
        // Pressing wins over touching
        assert_eq!(glow(&state, Button::RStickClick), 1.0);
        let state = pressing(&[Button::GripLTouch]);
        assert_eq!(glow(&state, Button::GripL2), TOUCHED);
        assert_eq!(glow(&state, Button::GripR1), 0.0);
    }

    #[test]
    fn test_glow_trigger() {
        let mut state = ControllerState::default();
        assert_eq!(glow(&state, Button::LtFull), 0.0);
        state.lt = i16::MAX as u16 / 2;
        assert!((glow(&state, Button::LtFull) - 0.5).abs() < 0.01);
        assert_eq!(glow(&state, Button::RtFull), 0.0);
        // Past the click the full bit lights it all the way
        state.rt = u16::MAX;
        assert_eq!(glow(&state, Button::RtFull), 1.0);
        state.buttons = Button::LtFull.mask().into();
        assert_eq!(glow(&state, Button::LtFull), 1.0);
    }

    #[test]
    fn test_lean() {
        let tip = |x, y| lean(Stick { x, y }) * Vec3::Z;
        assert!(tip(0, 0).abs_diff_eq(Vec3::Z, 1e-6));
        // Pushed right leans right, pushed up leans up
        assert!(tip(i16::MAX, 0).x > 0.3);
        assert!(tip(i16::MIN, 0).x < -0.3);
        assert!(tip(0, i16::MAX).y > 0.3);
        assert!(tip(0, i16::MIN).y < -0.3);
        assert!((tip(0, i16::MAX).y - STICK_LEAN.sin()).abs() < 1e-5);
    }

    #[test]
    fn test_dot() {
        let mut pad = Trackpad {
            x: i16::MAX,
            y: i16::MIN,
            ..Default::default()
        };
        assert_eq!(dot(pad), None);
        pad.touched = true;
        assert_eq!(dot(pad), Some(Vec2::new(1.0, -1.0)));
    }

    #[test]
    fn test_tint() {
        let alpha = |glow, selected, rest| tint(glow, selected, rest).0.alpha();
        assert_eq!(alpha(0.0, false, 0.0), 0.0);
        assert_eq!(alpha(0.0, false, 0.4), 0.4);
        assert!(alpha(1.0, false, 0.0) > 0.8);
        assert!(alpha(0.0, true, 0.0) > 0.5);
        // Glowing beats selected
        assert_eq!(tint(1.0, true, 0.0), tint(1.0, false, 0.0));
        assert_eq!(tint(0.0, false, 0.0).1, LinearRgba::BLACK);
        assert!(tint(1.0, false, 0.0).1.blue > tint(0.5, false, 0.0).1.blue);
    }

    #[test]
    fn test_sides() {
        let state = ControllerState {
            lstick: Stick { x: 1, y: 2 },
            rpad: Trackpad {
                x: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(stick(&state, Side::Left), Stick { x: 1, y: 2 });
        assert_eq!(stick(&state, Side::Right), Stick::default());
        assert_eq!(pad(&state, Side::Right).x, 3);
    }
}
