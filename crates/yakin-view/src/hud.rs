//! The text over the view: the connection, the active profile and the selected binding.

use bevy::prelude::*;

use yakin_view::model::Selection;

use crate::live::Live;

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn);
    app.add_systems(Update, update);
}

#[derive(Component, Clone, Debug)]
struct Hud;

fn spawn(mut commands: Commands) {
    commands.spawn((
        Hud,
        Text::new(""),
        Node {
            position_type: PositionType::Absolute,
            top: px(12),
            left: px(12),
            ..default()
        },
    ));
}

fn update(
    live: Res<Live>,
    selection: Res<Selection>,
    mut huds: Query<&mut Text, With<Hud>>,
) -> Result<()> {
    if !live.is_changed() && !selection.is_changed() {
        return Ok(());
    }
    let mut text = huds.single_mut()?;
    text.0 = lines(&live, selection.0);

    Ok(())
}

/// The HUD's text.
pub fn lines(
    live: &Live,
    selection: Option<yakin_core::state::Button>,
) -> String {
    let mut lines = vec![match (&live.down, live.controllers) {
        (Some(err), _) => format!("No daemon: {err}"),
        (None, 0) => "No controller".to_string(),
        (None, 1) => "1 controller".to_string(),
        (None, count) => format!("{count} controllers"),
    }];
    if !live.profile.is_empty() {
        lines.push(format!("Profile: {}", live.profile));
    }
    match selection {
        Some(button) => {
            lines.push(String::new());
            lines.push(button.to_string());
            lines.extend(
                live.binding(button)
                    .into_iter()
                    .map(|line| format!("  {line}")),
            );
        }
        None => lines.push("Click a control to see its binding".to_string()),
    }
    lines.join("\n")
}

#[cfg(test)]
mod test {
    use yakin_core::state::Button;
    use yakin_ipc::IpcEvent;

    use super::*;
    use crate::live::Feed;

    #[test]
    fn test_lines() {
        let mut live = Live::default();
        live.apply(Feed::Down("Cannot connect".to_string()));
        assert!(lines(&live, None).starts_with("No daemon: Cannot connect\n"));
        live.down = None;
        live.apply(Feed::Event(IpcEvent::Profile("pad".to_string())));
        let text = lines(&live, Some(Button::Lb));
        assert_eq!(text, "No controller\nProfile: pad\n\nlb\n  no config yet");
    }
}
