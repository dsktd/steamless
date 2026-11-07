//! The controller: Valve's shell, overlays on its controls, and the parts that move.

use std::path::Path;
use std::path::PathBuf;

use bevy::asset::io::embedded::EmbeddedAssetRegistry;
use bevy::prelude::*;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;
use yakin_core::state::Side;

use crate::layout;
use crate::layout::Shape;
use crate::layout::Zone;
use crate::look;

pub fn plugin(app: &mut App) {
    let path = shell_path();
    match std::fs::read(&path) {
        Ok(bytes) => app
            .world_mut()
            .resource_mut::<EmbeddedAssetRegistry>()
            .insert_asset(PathBuf::new(), Path::new("controller.glb"), bytes),
        Err(err) => warn!("No controller shell at {}: {err}", path.display()),
    }
    app.init_resource::<Input>();
    app.init_resource::<Selection>();
    app.add_systems(Startup, spawn);
    app.add_systems(Update, glow);
    app.add_systems(Update, lean);
    app.add_systems(Update, pull);
    app.add_systems(Update, touch);
}

/// Valve's shell, made at build time from its STL (cad/convert.py, see NOTICE): `$YAKIN_MODEL`,
/// else `share/yakin/controller.glb` in the package the binary runs from.
fn shell_path() -> PathBuf {
    if let Some(path) = std::env::var_os("YAKIN_MODEL") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let prefix = exe
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("/"));
    prefix.join("share/yakin/controller.glb")
}

/// The controller's latest state, which the overlays show.
#[derive(Resource, Clone, Debug, Default)]
pub struct Input(pub ControllerState);

/// The control last clicked.
#[derive(Resource, Clone, Debug, Default)]
pub struct Selection(pub Option<Button>);

/// An overlay that glows with `button`; `rest` is its opacity at rest.
#[derive(Component, Clone, Debug)]
#[require(Pickable)]
pub struct Lit {
    pub button: Button,
    rest: f32,
    material: Handle<StandardMaterial>,
}

/// Leans with the stick of its side.
#[derive(Component, Clone, Debug)]
struct Stick(Side);

/// Sinks with the trigger of its side from where it rests.
#[derive(Component, Clone, Debug)]
struct Trigger(Side, Vec3);

/// The finger on the trackpad of its side.
#[derive(Component, Clone, Debug)]
struct Dot(Side);

/// Overlays float this far over the surface, mm.
const LIFT: f32 = 0.6;
/// Overlay thickness, mm.
const THICKNESS: f32 = 1.0;

fn spawn(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let shell = GltfAssetLabel::Primitive {
        mesh: 0,
        primitive: 0,
    }
    .from_asset("embedded://controller.glb");
    commands.spawn((
        Mesh3d(assets.load(shell)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.16, 0.18),
            perceptual_roughness: 0.55,
            ..default()
        })),
        Pickable::IGNORE,
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 4000.0,
            ..default()
        },
        Transform::from_xyz(1.0, 3.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 400.0,
        ..default()
    });

    for zone in layout::ZONES {
        let mesh = meshes.add(plate(zone.shape));
        let mut transform = zone.transform();
        transform.translation += transform.rotation * Vec3::Z * mm(LIFT + THICKNESS / 2.0);
        let lit = lit(&mut materials, zone.button, 0.0);
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(lit.material.clone()),
            transform,
            lit,
        ));
    }

    for (side, button, [x, y, z]) in layout::STICKS {
        let top = layout::cad(x, y, z);
        let pivot = top - Vec3::Z * mm(layout::STICK_PIVOT);
        let mesh = meshes.add(plate(Shape::Round(layout::STICK_DIAMETER)));
        let lit = lit(&mut materials, button, 0.35);
        commands
            .spawn((
                Stick(side),
                Transform::from_translation(pivot),
                Visibility::default(),
            ))
            .with_children(|stick| {
                stick.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(lit.material.clone()),
                    Transform::from_xyz(0.0, 0.0, mm(layout::STICK_PIVOT + LIFT)),
                    lit,
                ));
            });
    }

    for (side, button, [x, y, z]) in layout::TRIGGERS {
        let (width, depth) = layout::TRIGGER_SIZE;
        let mesh = meshes.add(plate(Shape::Block(width, depth)));
        let rest = layout::cad(x, y, z) + Vec3::Y * mm(LIFT + THICKNESS / 2.0);
        let lit = lit(&mut materials, button, 0.35);
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(lit.material.clone()),
            Transform::from_translation(rest).with_rotation(layout::Facing::Top.rotation()),
            Trigger(side, rest),
            lit,
        ));
    }

    let dot = meshes.add(Sphere::new(mm(2.5)));
    let white = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        emissive: LinearRgba::rgb(3.0, 3.0, 3.0),
        unlit: true,
        ..default()
    });
    for side in [Side::Left, Side::Right] {
        commands.spawn((
            Mesh3d(dot.clone()),
            MeshMaterial3d(white.clone()),
            Transform::default(),
            Visibility::Hidden,
            Pickable::IGNORE,
            Dot(side),
        ));
    }
}

fn lit(
    materials: &mut Assets<StandardMaterial>,
    button: Button,
    rest: f32,
) -> Lit {
    let (base_color, emissive) = look::tint(0.0, false, rest);
    let material = materials.add(StandardMaterial {
        base_color,
        emissive,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    Lit {
        button,
        rest,
        material,
    }
}

/// A flat shape facing +Z.
fn plate(shape: Shape) -> Mesh {
    match shape {
        Shape::Round(diameter) => Mesh::from(Cylinder::new(mm(diameter) / 2.0, mm(THICKNESS)))
            .rotated_by(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        Shape::Block(width, height) => {
            Mesh::from(Cuboid::new(mm(width), mm(height), mm(THICKNESS)))
        }
    }
}

/// Millimetres in scene units.
fn mm(value: f32) -> f32 {
    value / 100.0
}

fn glow(
    input: Res<Input>,
    selection: Res<Selection>,
    lits: Query<&Lit>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !input.is_changed() && !selection.is_changed() {
        return;
    }
    for lit in &lits {
        let Some(mut material) = materials.get_mut(&lit.material) else {
            continue;
        };
        let glow = look::glow(&input.0, lit.button);
        let (base_color, emissive) = look::tint(glow, selection.0 == Some(lit.button), lit.rest);
        material.base_color = base_color;
        material.emissive = emissive;
    }
}

fn lean(
    input: Res<Input>,
    mut sticks: Query<(&Stick, &mut Transform)>,
) {
    for (stick, mut transform) in &mut sticks {
        transform.rotation = look::lean(look::stick(&input.0, stick.0));
    }
}

fn pull(
    input: Res<Input>,
    mut triggers: Query<(&Trigger, &mut Transform)>,
) {
    for (trigger, mut transform) in &mut triggers {
        let button = match trigger.0 {
            Side::Left => Button::LtFull,
            Side::Right => Button::RtFull,
        };
        let travel = look::glow(&input.0, button);
        transform.translation = trigger.1 - Vec3::Y * mm(layout::TRIGGER_TRAVEL) * travel;
    }
}

fn touch(
    input: Res<Input>,
    mut dots: Query<(&Dot, &mut Transform, &mut Visibility)>,
) {
    for (dot, mut transform, mut visibility) in &mut dots {
        let Some(at) = look::dot(look::pad(&input.0, dot.0)) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let zone: &Zone = layout::pad(dot.0);
        let Shape::Block(width, height) = zone.shape else {
            unreachable!()
        };
        let pad = zone.transform();
        let offset = Vec3::new(
            at.x * mm(width) / 2.0,
            at.y * mm(height) / 2.0,
            mm(LIFT + THICKNESS),
        );
        transform.translation = pad.translation + pad.rotation * offset;
        *visibility = Visibility::Visible;
    }
}
