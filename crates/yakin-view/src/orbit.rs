//! The camera circles the controller: drag to orbit, scroll to zoom, and it swings round to a
//! clicked control.

use std::f32::consts::PI;
use std::f32::consts::TAU;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::mouse::MouseScrollUnit;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn);
    app.add_systems(Update, drag);
    app.add_systems(Update, settle.after(drag));
}

/// Radians per pixel dragged.
const SPEED: f32 = 0.008;
/// Pointer travel, in pixels, past which a press is a drag and not a click.
pub const CLICK_SLOP: f32 = 4.0;
const DISTANCE: std::ops::RangeInclusive<f32> = 1.0..=8.0;
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.05;
/// How long the swing to a clicked control takes.
pub const SWING_SECONDS: f32 = 0.6;
/// How close the swing ends up.
const CLOSE: f32 = 1.5;

/// Where the camera is on its orbit.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Place {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// What the camera looks at.
    pub focus: Vec3,
}
impl Place {
    fn lerp(
        self,
        to: Place,
        k: f32,
    ) -> Place {
        Place {
            yaw: self.yaw + (to.yaw - self.yaw) * k,
            pitch: self.pitch + (to.pitch - self.pitch) * k,
            distance: self.distance + (to.distance - self.distance) * k,
            focus: self.focus.lerp(to.focus, k),
        }
    }
}

#[derive(Component, Clone, Debug)]
pub struct Orbit {
    pub place: Place,
    /// Pixels moved since the button went down.
    pub moved: f32,
    /// Where a swing started and ends, and how far along it is, 0 to 1.
    swing: Option<(Place, Place, f32)>,
}
impl Default for Orbit {
    fn default() -> Self {
        Self {
            place: Place {
                yaw: 0.0,
                pitch: -0.35,
                distance: 3.0,
                focus: Vec3::ZERO,
            },
            moved: 0.0,
            swing: None,
        }
    }
}
impl Orbit {
    /// Turn by a drag of `delta` pixels; the poles stay out of reach. A swing under way stops.
    pub fn turn(
        &mut self,
        delta: Vec2,
    ) {
        self.swing = None;
        self.place.yaw -= delta.x * SPEED;
        self.place.pitch = (self.place.pitch - delta.y * SPEED).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.moved += delta.length();
    }

    /// Zoom by scroll lines, positive toward the model.
    pub fn zoom(
        &mut self,
        lines: f32,
    ) {
        self.swing = None;
        self.place.distance =
            (self.place.distance * 0.9_f32.powf(lines)).clamp(*DISTANCE.start(), *DISTANCE.end());
    }

    /// Start a swing to a view of the control at `center` facing `normal`: along the normal,
    /// from a little above, the shortest way round.
    pub fn aim(
        &mut self,
        center: Vec3,
        normal: Vec3,
    ) {
        let yaw = normal.x.atan2(normal.z);
        let yaw = self.place.yaw + (yaw - self.place.yaw + PI).rem_euclid(TAU) - PI;
        let pitch = (-normal.y.asin() - 0.3).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let to = Place {
            yaw,
            pitch,
            distance: CLOSE,
            focus: center * 0.6,
        };
        self.swing = Some((self.place, to, 0.0));
    }

    /// Carry the swing on by `seconds`, eased.
    pub fn step(
        &mut self,
        seconds: f32,
    ) {
        let Some((from, to, t)) = &mut self.swing else {
            return;
        };
        *t = (*t + seconds / SWING_SECONDS).min(1.0);
        let k = *t * *t * (3.0 - 2.0 * *t);
        self.place = from.lerp(*to, k);
        if *t >= 1.0 {
            self.swing = None;
        }
    }

    pub fn transform(&self) -> Transform {
        let Place {
            yaw,
            pitch,
            distance,
            focus,
        } = self.place;
        let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        Transform::from_translation(focus + rotation * Vec3::Z * distance)
            .looking_at(focus, Vec3::Y)
    }

    pub fn is_drag(&self) -> bool {
        self.moved > CLICK_SLOP
    }
}

fn spawn(mut commands: Commands) {
    let orbit = Orbit::default();
    commands.spawn((Camera3d::default(), orbit.transform(), orbit));
}

fn drag(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut cameras: Query<&mut Orbit>,
) -> Result<()> {
    let mut orbit = cameras.single_mut()?;
    if buttons.just_pressed(MouseButton::Left) || buttons.just_pressed(MouseButton::Right) {
        orbit.moved = 0.0;
    }
    if buttons.pressed(MouseButton::Left) || buttons.pressed(MouseButton::Right) {
        orbit.turn(motion.delta);
    }
    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    if lines != 0.0 {
        orbit.zoom(lines);
    }

    Ok(())
}

/// Carry a swing on and put the camera where the orbit says.
pub fn settle(
    time: Res<Time>,
    mut cameras: Query<(&mut Orbit, &mut Transform)>,
) -> Result<()> {
    let (mut orbit, mut transform) = cameras.single_mut()?;
    orbit.step(time.delta_secs());
    *transform = orbit.transform();

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_orbit_front() {
        // The default looks at the face, from a little above
        let transform = Orbit::default().transform();
        assert!(transform.translation.z > 2.0);
        assert!(transform.translation.y > 0.0);
        assert!(transform.forward().dot(-transform.translation.normalize()) > 0.999);
    }

    #[test]
    fn test_orbit_turn() {
        let mut orbit = Orbit::default();
        orbit.turn(Vec2::new(0.0, -10_000.0));
        assert!(orbit.place.pitch < std::f32::consts::FRAC_PI_2);
        assert!(orbit.is_drag());
        orbit.turn(Vec2::new(std::f32::consts::PI / SPEED, 0.0));
        // Half way round: behind the controller
        orbit.place.pitch = 0.0;
        assert!(orbit.transform().translation.z < -2.9);
    }

    #[test]
    fn test_orbit_zoom() {
        let mut orbit = Orbit::default();
        orbit.zoom(1.0);
        assert!(orbit.place.distance < 3.0);
        orbit.zoom(1000.0);
        assert_eq!(orbit.place.distance, 1.0);
        orbit.zoom(-1000.0);
        assert_eq!(orbit.place.distance, 8.0);
    }

    #[test]
    fn test_orbit_aim() {
        let mut orbit = Orbit::default();
        // A control on the back: the camera ends up behind, looking at it, closer
        orbit.aim(Vec3::new(0.4, -0.3, -0.2), Vec3::NEG_Z);
        orbit.step(SWING_SECONDS / 4.0);
        assert!(
            orbit.transform().translation.z > 0.0,
            "a quarter in it is still in front"
        );
        orbit.step(SWING_SECONDS);
        let transform = orbit.transform();
        assert!(transform.translation.z < 0.0);
        assert!((orbit.place.distance - CLOSE).abs() < 1e-5);
        assert!(
            orbit
                .place
                .focus
                .abs_diff_eq(Vec3::new(0.24, -0.18, -0.12), 1e-5)
        );
        // Arrived: a further step changes nothing
        let place = orbit.place;
        orbit.step(1.0);
        assert_eq!(orbit.place, place);
    }

    #[test]
    fn test_orbit_aim_shortest_way() {
        let mut orbit = Orbit::default();
        orbit.place.yaw = 3.0;
        orbit.aim(Vec3::ZERO, Vec3::NEG_Z);
        orbit.step(SWING_SECONDS);
        // From 3.0 the back (yaw pi) is 0.14 on, not 6.1 back
        assert!((orbit.place.yaw - PI).abs() < 1e-5);
        assert!(orbit.place.yaw > 3.0);
    }

    #[test]
    fn test_orbit_drag_stops_swing() {
        let mut orbit = Orbit::default();
        orbit.aim(Vec3::ZERO, Vec3::NEG_Z);
        orbit.turn(Vec2::new(1.0, 0.0));
        let place = orbit.place;
        orbit.step(SWING_SECONDS);
        assert_eq!(orbit.place, place);
    }
}
