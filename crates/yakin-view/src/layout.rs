//! Where the controls sit on Valve's CAD model, measured in its millimetres.
//!
//! The STL is one solid, so the controls are placed by hand. Each centre was read off
//! orthographic renders of the STL's triangle centroids (front, back and top, a 10 mm grid),
//! and its height is the outermost centroid within 2 mm of that point: the largest Y for the
//! face, the smallest for the back, the smallest Z for the top edge.
//!
//! CAD axes: X to the right, Y out of the face, Z from the triggers toward the grips.

use bevy::prelude::*;
use yakin_core::profile::Axis;
use yakin_core::state::Button;
use yakin_core::state::Side;

/// The CAD's millimetres in the scene: Y up, Z toward the viewer, centred, 1 = 10 cm.
/// `cad/convert.py` converts the shell with the same map.
pub fn cad(
    x: f32,
    y: f32,
    z: f32,
) -> Vec3 {
    Vec3::new(x, 50.0 - z, y + 24.0) / 100.0
}

/// The way a control faces.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Facing {
    Front,
    Back,
    Top,
}
impl Facing {
    /// Turns +Z (a disc's and a block's face) to point this way.
    pub fn rotation(self) -> Quat {
        match self {
            Facing::Front => Quat::IDENTITY,
            Facing::Back => Quat::from_rotation_y(std::f32::consts::PI),
            Facing::Top => Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Shape {
    /// Diameter in mm.
    Round(f32),
    /// Width and height in mm, across the face.
    Block(f32, f32),
}

/// A clickable control and what lights it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Zone {
    pub button: Button,
    /// CAD mm, on the surface.
    pub at: [f32; 3],
    pub shape: Shape,
    pub facing: Facing,
    /// Extra tilt about the scene's X axis, radians, for sloped surfaces.
    pub pitch: f32,
}
impl Zone {
    const fn new(
        button: Button,
        at: [f32; 3],
        shape: Shape,
        facing: Facing,
    ) -> Self {
        Self {
            button,
            at,
            shape,
            facing,
            pitch: 0.0,
        }
    }

    pub fn transform(&self) -> Transform {
        let [x, y, z] = self.at;
        Transform::from_translation(cad(x, y, z))
            .with_rotation(Quat::from_rotation_x(self.pitch) * self.facing.rotation())
    }
}

/// The trackpads fall about 6 mm over their 22 mm of height toward the grips.
const PAD_PITCH: f32 = 0.26;

/// The D-pad's centre; its arms reach 7 mm out.
const DPAD: [f32; 3] = [-49.4, 1.0, 19.7];

/// Every control drawn as a flat overlay. The sticks and triggers move, see [`STICKS`] and
/// [`TRIGGERS`].
pub const ZONES: [Zone; 20] = [
    Zone::new(
        Button::A,
        [48.9, 0.8, 28.7],
        Shape::Round(9.0),
        Facing::Front,
    ),
    Zone::new(
        Button::B,
        [58.1, -0.3, 19.5],
        Shape::Round(9.0),
        Facing::Front,
    ),
    Zone::new(
        Button::X,
        [39.9, 1.6, 19.5],
        Shape::Round(9.0),
        Facing::Front,
    ),
    Zone::new(
        Button::Y,
        [48.9, 0.7, 10.2],
        Shape::Round(9.0),
        Facing::Front,
    ),
    Zone::new(
        Button::Steam,
        [0.1, 0.8, 19.2],
        Shape::Round(11.0),
        Facing::Front,
    ),
    Zone::new(
        Button::View,
        [-28.4, 0.5, 8.2],
        Shape::Block(10.0, 4.5),
        Facing::Front,
    ),
    Zone::new(
        Button::Menu,
        [28.1, 0.5, 8.2],
        Shape::Block(10.0, 4.5),
        Facing::Front,
    ),
    Zone::new(
        Button::Quick,
        [0.1, -5.1, 61.7],
        Shape::Block(13.0, 5.0),
        Facing::Front,
    ),
    Zone::new(
        Button::DpadUp,
        [DPAD[0], DPAD[1], DPAD[2] - 7.0],
        Shape::Block(7.0, 7.0),
        Facing::Front,
    ),
    Zone::new(
        Button::DpadDown,
        [DPAD[0], DPAD[1], DPAD[2] + 7.0],
        Shape::Block(7.0, 7.0),
        Facing::Front,
    ),
    Zone::new(
        Button::DpadLeft,
        [DPAD[0] - 7.0, DPAD[1], DPAD[2]],
        Shape::Block(7.0, 7.0),
        Facing::Front,
    ),
    Zone::new(
        Button::DpadRight,
        [DPAD[0] + 7.0, DPAD[1], DPAD[2]],
        Shape::Block(7.0, 7.0),
        Facing::Front,
    ),
    Zone {
        pitch: PAD_PITCH,
        ..Zone::new(
            Button::LPadClick,
            [-29.9, -6.2, 61.2],
            Shape::Block(34.0, 32.0),
            Facing::Front,
        )
    },
    Zone {
        pitch: PAD_PITCH,
        ..Zone::new(
            Button::RPadClick,
            [29.9, -6.2, 61.2],
            Shape::Block(34.0, 32.0),
            Facing::Front,
        )
    },
    Zone::new(
        Button::Lb,
        [-52.0, -11.0, -4.8],
        Shape::Block(20.0, 10.0),
        Facing::Top,
    ),
    Zone::new(
        Button::Rb,
        [52.0, -11.0, -4.8],
        Shape::Block(20.0, 10.0),
        Facing::Top,
    ),
    // The upper paddles are taken as 1, the lower as 2
    Zone::new(
        Button::GripL1,
        [-42.6, -42.7, 53.5],
        Shape::Round(13.0),
        Facing::Back,
    ),
    Zone::new(
        Button::GripL2,
        [-48.1, -52.1, 74.7],
        Shape::Round(13.0),
        Facing::Back,
    ),
    Zone::new(
        Button::GripR1,
        [41.9, -41.6, 53.5],
        Shape::Round(13.0),
        Facing::Back,
    ),
    Zone::new(
        Button::GripR2,
        [48.9, -53.1, 74.7],
        Shape::Round(13.0),
        Facing::Back,
    ),
];

/// Stick caps: their top in CAD mm and diameter; the cap tilts about a pivot under the face.
pub const STICKS: [(Side, Button, [f32; 3]); 2] = [
    (Side::Left, Button::LStickClick, [-23.1, 7.3, 31.0]),
    (Side::Right, Button::RStickClick, [22.6, 7.3, 31.0]),
];
pub const STICK_DIAMETER: f32 = 19.0;
/// How far under the cap's top the stick pivots, mm.
pub const STICK_PIVOT: f32 = 14.0;

/// Triggers on the top edge, behind the bumpers; they sink toward the grips as they travel.
pub const TRIGGERS: [(Side, Button, [f32; 3]); 2] = [
    (Side::Left, Button::LtFull, [-52.0, -28.0, 0.5]),
    (Side::Right, Button::RtFull, [52.0, -28.0, 0.5]),
];
pub const TRIGGER_SIZE: (f32, f32) = (20.0, 14.0);
/// mm a fully pulled trigger sinks.
pub const TRIGGER_TRAVEL: f32 = 6.0;

/// The axis a clickable control also moves.
pub fn axis(button: Button) -> Option<Axis> {
    match button {
        Button::LStickClick => Some(Axis::LStick),
        Button::RStickClick => Some(Axis::RStick),
        Button::LPadClick => Some(Axis::LPad),
        Button::RPadClick => Some(Axis::RPad),
        Button::LtFull => Some(Axis::Lt),
        Button::RtFull => Some(Axis::Rt),
        _ => None,
    }
}

/// Pads for the touch dot, by side: the click zone's centre and size.
pub fn pad(side: Side) -> &'static Zone {
    let button = match side {
        Side::Left => Button::LPadClick,
        Side::Right => Button::RPadClick,
    };
    ZONES.iter().find(|zone| zone.button == button).unwrap()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_cad() {
        // The trigger edge is up, the face toward the viewer
        assert!(cad(0.0, 0.0, -5.0).y > cad(0.0, 0.0, 100.0).y);
        assert!(cad(0.0, 8.0, 0.0).z > cad(0.0, -56.0, 0.0).z);
        assert_eq!(cad(10.0, -24.0, 50.0), Vec3::new(0.1, 0.0, 0.0));
    }

    #[test]
    fn test_facing() {
        let out = |facing: Facing| facing.rotation() * Vec3::Z;
        assert!(out(Facing::Front).abs_diff_eq(Vec3::Z, 1e-6));
        assert!(out(Facing::Back).abs_diff_eq(Vec3::NEG_Z, 1e-6));
        assert!(out(Facing::Top).abs_diff_eq(Vec3::Y, 1e-6));
    }

    #[test]
    fn test_zones_unique() {
        for (i, zone) in ZONES.iter().enumerate() {
            assert!(
                ZONES[i + 1..]
                    .iter()
                    .all(|other| other.button != zone.button),
                "{} twice",
                zone.button
            );
        }
    }
}
