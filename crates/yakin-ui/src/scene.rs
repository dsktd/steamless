//! The scene thread: Bevy without a window, drawing yakin-view's controller into a texture on
//! Blitz's wgpu device, steered by the page's requests.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::time::Duration;
use std::time::Instant;

use bevy::app::PluginsState;
use bevy::camera::RenderTarget;
use bevy::picking::mesh_picking::ray_cast::MeshRayCast;
use bevy::picking::mesh_picking::ray_cast::MeshRayCastSettings;
use bevy::prelude::*;
use bevy::render::Render;
use bevy::render::RenderApp;
use bevy::render::RenderPlugin;
use bevy::render::RenderSystems;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::extract_resource::ExtractResourcePlugin;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::TextureFormat;
use bevy::render::render_resource::TextureId;
use bevy::render::renderer::RenderAdapter;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::render::renderer::RenderDevice;
use bevy::render::renderer::RenderInstance;
use bevy::render::renderer::RenderQueue;
use bevy::render::settings::RenderCreation;
use bevy::render::texture::GpuImage;
use bevy::window::ExitCondition;
use dioxus_native::DeviceHandle;
use tokio::sync::mpsc::UnboundedSender;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;
use yakin_view::look;
use yakin_view::model;
use yakin_view::model::Input;
use yakin_view::model::Lit;
use yakin_view::model::Selection;
use yakin_view::orbit;
use yakin_view::orbit::Orbit;

use crate::view::Pending;
use crate::view::Request;

/// The scene's ends of the widget's channels.
pub struct Ends {
    pub requests: Pending,
    /// Every target texture made, for the widget to show.
    pub textures: Sender<wgpu::TextureView>,
    /// Every control clicked on the model.
    pub picks: UnboundedSender<Option<Button>>,
    /// A tick for every paint of the widget.
    pub paints: Receiver<()>,
}

#[derive(Resource)]
struct Link {
    requests: Mutex<Pending>,
    picks: UnboundedSender<Option<Button>>,
}

/// The target image, and where its texture goes once the render world has it.
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
struct Target {
    image: Handle<Image>,
    textures: Arc<Mutex<(Sender<wgpu::TextureView>, Option<TextureId>)>>,
}

pub fn run(
    handle: DeviceHandle,
    ends: Ends,
) {
    let DeviceHandle {
        instance,
        adapter,
        device,
        queue,
    } = handle;
    let render = RenderCreation::manual(
        RenderDevice::from(device),
        RenderQueue::new(queue),
        RenderAdapterInfo::new(adapter.get_info()),
        RenderAdapter::new(adapter),
        RenderInstance::new(instance),
    );
    let mut app = App::new();
    // The window's env_logger is the logger; Bevy's would fail to replace it
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<bevy::log::LogPlugin>()
            .set(RenderPlugin {
                render_creation: render,
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            }),
    );
    app.set_runner(move |app| run_paced(app, ends.paints));
    app.add_plugins(ExtractResourcePlugin::<Target>::default());
    app.add_plugins(model::plugin);
    app.insert_resource(Link {
        requests: Mutex::new(ends.requests),
        picks: ends.picks,
    });
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(target_image(640, 480));
    app.insert_resource(Target {
        image,
        textures: Arc::new(Mutex::new((ends.textures, None))),
    });
    app.add_systems(Startup, spawn);
    app.add_systems(Update, requests);
    app.add_systems(Update, orbit::settle.after(requests));
    app.sub_app_mut(RenderApp)
        .add_systems(Render, publish.after(RenderSystems::PrepareAssets));
    app.run();
}

/// How long between frames while the scene draws.
const FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Whether the scene draws: only while the widget paints, and Blitz paints it only while the 3D
/// tab is in the page and the window is on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pace {
    painted: Option<Instant>,
}
impl Pace {
    /// How long the scene keeps drawing after the last paint.
    pub const LINGER: Duration = Duration::from_millis(250);

    pub fn painted(
        &mut self,
        at: Instant,
    ) {
        self.painted = Some(at);
    }

    pub fn awake(
        &self,
        now: Instant,
    ) -> bool {
        self.painted
            .is_some_and(|at| now.saturating_duration_since(at) < Self::LINGER)
    }
}

/// Bevy's loop, at 60 fps while the widget paints; asleep until its next paint otherwise, so a
/// hidden tab costs nothing and the world waits as it was.
fn run_paced(
    mut app: App,
    paints: Receiver<()>,
) -> AppExit {
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let mut pace = Pace::default();
    loop {
        if paints.try_iter().count() > 0 {
            pace.painted(Instant::now());
        }
        if !pace.awake(Instant::now()) {
            // The widget is gone with the window when the channel is
            if paints.recv().is_err() {
                return AppExit::Success;
            }
            pace.painted(Instant::now());
        }
        let start = Instant::now();
        app.update();
        if let Some(exit) = app.should_exit() {
            return exit;
        }
        std::thread::sleep(FRAME.saturating_sub(start.elapsed()));
    }
}

/// A render target Blitz can sample as it is: Bevy writes it sRGB-encoded, the Unorm view
/// skips a second decode.
fn target_image(
    width: u32,
    height: u32,
) -> Image {
    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.view_formats = &[TextureFormat::Rgba8Unorm];
    image
}

fn spawn(
    mut commands: Commands,
    target: Res<Target>,
) {
    let orbit = Orbit::default();
    commands.spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.11, 0.11, 0.13)),
            ..default()
        },
        RenderTarget::Image(target.image.clone().into()),
        orbit.transform(),
        orbit,
    ));
}

/// Render world: hand the widget the target's texture whenever it is a new one.
fn publish(
    target: Res<Target>,
    images: Res<RenderAssets<GpuImage>>,
) {
    let Some(gpu) = images.get(&target.image) else {
        return;
    };
    let mut slot = target.textures.lock().unwrap();
    if slot.1 == Some(gpu.texture.id()) {
        return;
    }
    slot.1 = Some(gpu.texture.id());
    let raw: &wgpu::Texture = &gpu.texture;
    let view = raw.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    let _ = slot.0.send(view);
}

#[allow(clippy::too_many_arguments)]
fn requests(
    link: Res<Link>,
    mut target: ResMut<Target>,
    mut images: ResMut<Assets<Image>>,
    mut render_targets: Query<&mut RenderTarget, With<Camera3d>>,
    mut input: ResMut<Input>,
    mut selection: ResMut<Selection>,
    mut cameras: Query<(&Camera, &GlobalTransform, &mut Orbit)>,
    lits: Query<(Entity, &Lit, &GlobalTransform)>,
    mut ray_cast: MeshRayCast,
    time: Res<Time>,
) -> Result<()> {
    let (camera, camera_transform, mut orbit) = cameras.single_mut()?;
    let requests = link.requests.lock().unwrap().take();
    for request in requests {
        let picked = match request {
            Request::Resize(width, height) => {
                // A new image on a new handle, and the camera pointed at it: Bevy sees the target
                // change in the same frame, so its depth texture and viewport follow the color
                // one. Rewriting the old handle in place left them a frame apart.
                let old =
                    std::mem::replace(&mut target.image, images.add(target_image(width, height)));
                for mut render_target in &mut render_targets {
                    *render_target = RenderTarget::Image(target.image.clone().into());
                }
                images.remove(&old);
                continue;
            }
            Request::Drag(dx, dy) => {
                orbit.turn(Vec2::new(dx, dy));
                continue;
            }
            Request::Zoom(lines) => {
                orbit.zoom(lines);
                continue;
            }
            Request::State(state) => {
                if input.0 != state {
                    input.0 = state;
                }
                continue;
            }
            Request::Select(button) => button,
            Request::Click(x, y) => {
                let Some(size) = camera.logical_viewport_size() else {
                    continue;
                };
                let Ok(ray) = camera.viewport_to_world(camera_transform, Vec2::new(x, y) * size)
                else {
                    continue;
                };
                // The overlay under the pointer, else the one nearest the point hit on the shell
                let hits = ray_cast.cast_ray(ray, &MeshRayCastSettings::default());
                match hits.first() {
                    None => None,
                    Some((entity, hit)) => match lits.get(*entity) {
                        Ok((_, lit, _)) => Some(lit.button),
                        Err(_) => nearest(
                            hit.point,
                            lits.iter()
                                .map(|(_, lit, at)| (lit.button, at.translation())),
                        ),
                    },
                }
            }
        };
        if selection.0 != picked {
            selection.0 = picked;
        }
        let overlays = lits
            .iter()
            .map(|(_, lit, at)| (lit.button, at.translation(), at.rotation()));
        if let Some((center, normal)) = aim(picked, overlays) {
            orbit.aim(center, normal);
        }
        if matches!(request, Request::Click(..)) {
            let _ = link.picks.send(picked);
        }
    }
    // The right stick and the triggers, held, keep turning and zooming
    let (turn, zoom) = steer(&input.0, time.delta_secs());
    if turn != Vec2::ZERO {
        orbit.turn(turn);
    }
    if zoom != 0.0 {
        orbit.zoom(zoom);
    }

    Ok(())
}

/// Where the camera swings for `picked`: its overlay's centre and the way the overlay faces.
pub fn aim(
    picked: Option<Button>,
    mut overlays: impl Iterator<Item = (Button, Vec3, Quat)>,
) -> Option<(Vec3, Vec3)> {
    let picked = picked?;
    overlays
        .find(|(button, _, _)| *button == picked)
        .map(|(_, at, rotation)| (at, rotation * Vec3::Z))
}

/// Stick or trigger travel under this is rest, 0 to 1.
const STEER_DEADZONE: f32 = 0.15;
/// Drag pixels a second at full deflection of the right stick.
const STEER_TURN: f32 = 400.0;
/// Scroll lines a second at a full pull of a trigger.
const STEER_ZOOM: f32 = 6.0;

/// The right stick's turn, as a drag, and the triggers' zoom, as scroll lines, over `seconds`:
/// pushed up looks from above, the right trigger zooms in and the left out.
pub fn steer(
    state: &ControllerState,
    seconds: f32,
) -> (Vec2, f32) {
    let live = |value: f32| {
        if value.abs() < STEER_DEADZONE {
            0.0
        } else {
            value
        }
    };
    let axis = |value: i16| live((value as f32 / i16::MAX as f32).clamp(-1.0, 1.0));
    let turn = Vec2::new(axis(state.rstick.x), axis(state.rstick.y)) * STEER_TURN * seconds;
    let zoom = (live(look::travel(state.rt)) - live(look::travel(state.lt))) * STEER_ZOOM * seconds;
    (turn, zoom)
}

/// The control whose overlay is nearest `point`.
pub fn nearest(
    point: Vec3,
    overlays: impl Iterator<Item = (Button, Vec3)>,
) -> Option<Button> {
    overlays
        .min_by(|a, b| {
            a.1.distance_squared(point)
                .total_cmp(&b.1.distance_squared(point))
        })
        .map(|(button, _)| button)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_pace() {
        let start = Instant::now();
        let mut pace = Pace::default();
        assert!(!pace.awake(start), "asleep before the first paint");
        pace.painted(start);
        assert!(pace.awake(start));
        assert!(pace.awake(start + Pace::LINGER / 2));
        assert!(!pace.awake(start + Pace::LINGER), "asleep once paints stop");
        pace.painted(start + Pace::LINGER * 4);
        assert!(
            pace.awake(start + Pace::LINGER * 4),
            "awake at the next paint"
        );
    }

    #[test]
    fn test_list_picks_aim_the_camera() {
        // Every control in the list has an overlay to swing to, as a click on it does
        let list = crate::model::controls();
        let overlays = || {
            yakin_view::layout::ZONES
                .iter()
                .map(|zone| {
                    let at = zone.transform();
                    (zone.button, at.translation, at.rotation)
                })
                .chain(
                    yakin_view::layout::STICKS
                        .iter()
                        .chain(yakin_view::layout::TRIGGERS.iter())
                        .map(|(_, button, [x, y, z])| {
                            (*button, yakin_view::layout::cad(*x, *y, *z), Quat::IDENTITY)
                        }),
                )
        };
        for button in &list {
            assert!(aim(Some(*button), overlays()).is_some(), "{button:?}");
        }
        let first = crate::model::step(&list, None, true);
        let zone = &yakin_view::layout::ZONES[0];
        assert_eq!(
            aim(first, overlays()),
            Some((zone.transform().translation, Vec3::Z)),
            "the first step swings to the first zone, face on"
        );
        assert_eq!(aim(None, overlays()), None);
    }

    #[test]
    fn test_steer() {
        let mut state = ControllerState::default();
        assert_eq!(steer(&state, 1.0), (Vec2::ZERO, 0.0), "at rest");
        state.rstick.x = i16::MAX / 10;
        assert_eq!(steer(&state, 1.0), (Vec2::ZERO, 0.0), "inside the deadzone");
        state.rstick.x = i16::MAX;
        state.rstick.y = i16::MAX;
        assert_eq!(
            steer(&state, 0.5).0,
            Vec2::new(STEER_TURN, STEER_TURN) / 2.0,
            "up raises the camera, as a drag down does"
        );
        state.rt = i16::MAX as u16;
        assert_eq!(
            steer(&state, 1.0).1,
            STEER_ZOOM,
            "the right trigger zooms in"
        );
        state.lt = i16::MAX as u16;
        assert_eq!(steer(&state, 1.0).1, 0.0, "both cancel");
    }

    #[test]
    fn test_nearest() {
        let overlays = [
            (Button::A, Vec3::new(1.0, 0.0, 0.0)),
            (Button::B, Vec3::new(0.0, 2.0, 0.0)),
            (Button::X, Vec3::new(0.0, 0.0, -3.0)),
        ];
        assert_eq!(
            nearest(Vec3::new(0.0, 1.5, 0.0), overlays.into_iter()),
            Some(Button::B)
        );
        assert_eq!(
            nearest(Vec3::new(0.0, 0.0, -9.0), overlays.into_iter()),
            Some(Button::X)
        );
        assert_eq!(nearest(Vec3::ZERO, std::iter::empty()), None);
    }
}
