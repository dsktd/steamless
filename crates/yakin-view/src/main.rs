//! A 3D Steam Controller that shows the daemon's live input and the binding of a clicked button.

mod hud;
mod live;

use std::path::PathBuf;

use bevy::prelude::*;
use clap::Parser;
use yakin_view::model;
use yakin_view::model::Lit;
use yakin_view::model::Selection;
use yakin_view::orbit;
use yakin_view::orbit::Orbit;

use crate::live::Live;

#[derive(Parser)]
#[command(about = "A 3D Steam Controller with the daemon's live input")]
struct Args {
    /// The daemon's socket; default: the user's daemon, else the system one
    #[arg(long, env = "YAKIN_SOCKET")]
    socket: Option<PathBuf>,
}

fn main() -> AppExit {
    let args = Args::parse();
    let socket = args.socket.unwrap_or_else(yakin_ipc::path::client_socket);

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Yakin".to_string(),
                name: Some("dev.dsktd.yakin.view".to_string()),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(MeshPickingPlugin)
        .insert_resource(live::Socket(socket))
        .add_plugins(hud::plugin)
        .add_plugins(live::plugin)
        .add_plugins(model::plugin)
        .add_plugins(orbit::plugin)
        .add_observer(on_click)
        .run()
}

/// A click on an overlay selects its control and turns the camera to it.
fn on_click(
    click: On<PointerClick>,
    lits: Query<(&Lit, &GlobalTransform)>,
    mut cameras: Query<&mut Orbit>,
    live: Res<Live>,
    mut selection: ResMut<Selection>,
) -> Result<()> {
    let mut orbit = cameras.single_mut()?;
    if click.button != PointerButton::Primary || orbit.is_drag() {
        return Ok(());
    }
    let (lit, transform) = lits.get(click.entity)?;
    selection.0 = Some(lit.button);
    orbit.aim(transform.translation(), transform.rotation() * Vec3::Z);
    println!(
        "{} ({}): {}",
        lit.button,
        profile(&live),
        live.binding(lit.button).join(", ")
    );

    Ok(())
}

fn profile(live: &Live) -> &str {
    match live.profile.is_empty() {
        true => "active profile",
        false => &live.profile,
    }
}
