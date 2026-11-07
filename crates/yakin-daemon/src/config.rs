//! The config as layers: a baseline file, then every `*.toml` of a drop-in directory by name.
//! Editors write their changes to one drop-in, [`EDITOR`], as the difference to the rest.

use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::anyhow;
use toml::Table;
use yakin_core::Result;
use yakin_core::layer;
use yakin_core::profile::Config;
use yakin_core::profile::ConfigToml;
use yakin_ipc::IpcConfig;
use yakin_ipc::IpcLayer;
use yakin_ipc::IpcTuning;

/// The drop-in editors write.
pub const EDITOR: &str = "90-editor.toml";

/// One file of the config and what it says.
pub struct Source {
    pub path: PathBuf,
    pub table: Table,
}

pub struct Layers {
    pub baseline: Option<PathBuf>,
    pub dir: PathBuf,
}
impl Layers {
    pub fn editor(&self) -> PathBuf {
        self.dir.join(EDITOR)
    }

    /// The baseline, when it exists, then the drop-ins sorted by file name.
    pub fn read(&self) -> Result<Vec<Source>> {
        let mut paths: Vec<PathBuf> = self
            .baseline
            .iter()
            .filter(|path| path.exists())
            .cloned()
            .collect();
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => {
                let mut dropins: Vec<PathBuf> = entries
                    .filter_map(|entry| Some(entry.ok()?.path()))
                    .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
                    .collect();
                dropins.sort();
                paths.extend(dropins);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(anyhow!("Cannot read {}: {err}", self.dir.display())),
        }
        paths
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path)
                    .map_err(|err| anyhow!("Cannot read config {}: {err}", path.display()))?;
                let table =
                    layer::parse(&text).map_err(|err| anyhow!("{}: {err}", path.display()))?;
                Ok(Source { path, table })
            })
            .collect()
    }

    /// Every layer merged and checked as a whole.
    pub fn load(&self) -> Result<(ConfigToml, Config)> {
        merged(self.read()?)
    }

    /// The layers as `GetConfig` answers them.
    pub fn ipc_config(&self) -> Result<IpcConfig> {
        let sources = self.read()?;
        let mut baseline = None;
        let mut dropins = Vec::new();
        for source in sources.iter() {
            let layer = IpcLayer {
                path: source.path.display().to_string(),
                toml: layer::from_table(source.table.clone())
                    .map_err(|err| anyhow!("{}: {err}", source.path.display()))?,
            };
            if Some(&source.path) == self.baseline.as_ref() {
                baseline = Some(layer);
            } else {
                dropins.push(layer);
            }
        }
        let (effective, _) = merged(sources)?;
        Ok(IpcConfig {
            baseline,
            dropins,
            effective,
            editor_path: self.editor().display().to_string(),
            writable: self.writable(),
        })
    }

    /// Whether [`EDITOR`] can be written, or its directory created.
    pub fn writable(&self) -> bool {
        let Some(dir) = self.dir.ancestors().find(|dir| dir.exists()) else {
            return false;
        };
        if dir
            .canonicalize()
            .is_ok_and(|real| real.starts_with("/nix/store"))
            || std::fs::metadata(dir).is_ok_and(|meta| meta.permissions().readonly())
        {
            return false;
        }
        let editor = self.editor();
        if editor.exists() {
            return std::fs::OpenOptions::new()
                .append(true)
                .open(&editor)
                .is_ok();
        }
        let probe = dir.join(format!(".yakin.{}.probe", std::process::id()));
        let ok = std::fs::write(&probe, "").is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
    }

    /// Write to [`EDITOR`] what `toml` changes over the other layers, through a temp file.
    /// The merged result is checked first; nothing is written when it fails. The leading
    /// comment block of the old file stays.
    pub fn save(
        &self,
        toml: &ConfigToml,
    ) -> Result<Config> {
        let path = self.editor();
        let others: Vec<Source> = self
            .read()?
            .into_iter()
            .filter(|source| source.path != path)
            .collect();
        let mut under = Table::new();
        for source in others.iter() {
            layer::merge(&mut under, source.table.clone());
        }
        let changes = layer::diff(&under, &layer::to_table(toml)?);
        let mut all = under;
        layer::merge(&mut all, changes.clone());
        let config = Config::try_from(layer::from_table(all)?)?;
        // The daemon must still start with it
        config.profile()?;
        let read_only = || anyhow!("config is read-only: {}", self.dir.display());
        if !self.writable() {
            return Err(read_only());
        }
        let denied = |err: std::io::Error| match err.kind() {
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => read_only(),
            _ => anyhow!("Cannot write {}: {err}", path.display()),
        };
        if !self.dir.exists() {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o750)
                .create(&self.dir)
                .map_err(denied)?;
        }
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let text = format!("{}{}", header(&old), toml::to_string(&changes)?);
        let temp = self
            .dir
            .join(format!(".{EDITOR}.{}.tmp", std::process::id()));
        if let Err(err) = std::fs::write(&temp, text) {
            let _ = std::fs::remove_file(&temp);
            return Err(denied(err));
        }
        if let Err(err) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(denied(err));
        }
        Ok(config)
    }

    /// Write the knobs of `tunings` into [`EDITOR`] over the current layers, through
    /// [`Layers::save`]; everything else stays as the layers have it.
    pub fn save_tuning(
        &self,
        tunings: &[IpcTuning],
    ) -> Result<Config> {
        let (mut toml, config) = self.load()?;
        for tuning in tunings {
            // Knobs at what the layers already give stay unwritten, defaults too
            let was = crate::engine::tuning(&tuning.profile, &config.get(&tuning.profile)?);
            let set = |new: f32, old: f32, field: &mut Option<f32>| {
                if new != old {
                    *field = Some(new);
                }
            };
            let profile = toml.profile.entry(tuning.profile.clone()).or_default();
            set(
                tuning.mouse_speed,
                was.mouse_speed,
                &mut profile.mouse.speed,
            );
            set(
                tuning.mouse_accel,
                was.mouse_accel,
                &mut profile.mouse.accel,
            );
            set(
                tuning.mouse_smoothing,
                was.mouse_smoothing,
                &mut profile.mouse.smoothing,
            );
            set(
                tuning.mouse_stick_speed,
                was.mouse_stick_speed,
                &mut profile.mouse.stick_speed,
            );
            set(
                tuning.mouse_stick_curve,
                was.mouse_stick_curve,
                &mut profile.mouse.stick_curve,
            );
            set(
                tuning.mouse_flick_degrees,
                was.mouse_flick_degrees,
                &mut profile.mouse.flick_degrees,
            );
            set(
                tuning.scroll_speed,
                was.scroll_speed,
                &mut profile.scroll.speed,
            );
            set(
                tuning.scroll_stick_speed,
                was.scroll_stick_speed,
                &mut profile.scroll.stick_speed,
            );
            set(
                tuning.deadzone_stick,
                was.deadzone_stick,
                &mut profile.deadzone.stick,
            );
            set(
                tuning.deadzone_trigger,
                was.deadzone_trigger,
                &mut profile.deadzone.trigger,
            );
            set(
                tuning.trigger_click,
                was.trigger_click,
                &mut profile.trigger.click,
            );
            set(
                tuning.haptics_click,
                was.haptics_click,
                &mut profile.haptics.click,
            );
            set(
                tuning.haptics_pad_ticks,
                was.haptics_pad_ticks,
                &mut profile.haptics.pad_ticks,
            );
            set(
                tuning.haptics_menu,
                was.haptics_menu,
                &mut profile.haptics.menu,
            );
            if tuning.mouse_release_guard != was.mouse_release_guard {
                profile.mouse.release_guard = Some(tuning.mouse_release_guard);
            }
            if tuning.scroll_natural != was.scroll_natural {
                profile.scroll.natural = Some(tuning.scroll_natural);
            }
        }
        self.save(&toml)
    }
}

/// `sources` merged in order, as written and checked.
pub fn merged(sources: Vec<Source>) -> Result<(ConfigToml, Config)> {
    let mut all = Table::new();
    for source in sources {
        layer::merge(&mut all, source.table);
    }
    let toml = layer::from_table(all)?;
    Ok((toml.clone(), Config::try_from(toml)?))
}

/// The comment lines at the top of a file, with the blank line after them.
fn header(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!("{}\n\n", lines.join("\n"))
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use super::*;

    /// The window's test fixtures: the Vocabulary and a GetConfig answer for the examples, as the
    /// daemon builds them. `YAKIN_FIXTURES=update` (`just ui-fixtures`) rewrites the files.
    #[test]
    fn test_ui_fixtures_match() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let layers = Layers {
            baseline: Some(root.join("examples/settop.toml")),
            dir: root.join("examples/config.d"),
        };
        let config = serde_json::to_string_pretty(&layers.ipc_config().unwrap()).unwrap();
        // The checkout's location stays out of the files; the editor's drop-in does not exist
        let config = config.replace(root.to_str().unwrap(), "/yakin");
        let vocabulary =
            serde_json::to_string_pretty(&yakin_core::profile::Vocabulary::new()).unwrap();
        let dir = root.join("crates/yakin-ui/tests/fixtures");
        for (name, text) in [("config.json", config), ("vocabulary.json", vocabulary)] {
            let path = dir.join(name);
            let text = format!("{text}\n");
            if std::env::var("YAKIN_FIXTURES").is_ok_and(|mode| mode == "update") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &text).unwrap();
            }
            assert_eq!(
                std::fs::read_to_string(&path).unwrap_or_default(),
                text,
                "{} is stale: run `just ui-fixtures`",
                path.display()
            );
        }
    }

    fn layers(name: &str) -> Layers {
        let dir = std::env::temp_dir().join(format!("yakin-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Layers {
            baseline: Some(dir.join("config.toml")),
            dir: dir.join("config.d"),
        }
    }

    fn root(layers: &Layers) -> PathBuf {
        layers.dir.parent().unwrap().to_path_buf()
    }

    const BASE: &str = "[profile.p.buttons]\na = \"mouse:left\"\nx = \"pad:west\"\n";

    #[test]
    fn test_save_writes_only_the_diff() {
        let layers = layers("diff");
        std::fs::write(layers.baseline.as_ref().unwrap(), BASE).unwrap();
        let (mut toml, _) = layers.load().unwrap();
        let buttons = &mut toml.profile.get_mut("p").unwrap().buttons;
        buttons.insert("x".into(), "key:KEY_X".into());
        // Creates the directory and the file
        layers.save(&toml).unwrap();
        let text = std::fs::read_to_string(layers.editor()).unwrap();
        assert_eq!(text, "[profile.p.buttons]\nx = \"key:KEY_X\"\n");
        assert_eq!(layers.load().unwrap().0, toml);
        // Keeps the comment block; equal to the baseline again leaves an empty file
        std::fs::write(layers.editor(), format!("# mine\n{text}")).unwrap();
        let (toml, _) = merged(vec![Source {
            path: layers.baseline.clone().unwrap(),
            table: layer::parse(BASE).unwrap(),
        }])
        .unwrap();
        layers.save(&toml).unwrap();
        assert_eq!(
            std::fs::read_to_string(layers.editor()).unwrap(),
            "# mine\n\n"
        );
        std::fs::remove_dir_all(root(&layers)).unwrap();
    }

    #[test]
    fn test_save_rejects() {
        let layers = layers("reject");
        let mut toml = ConfigToml::parse(include_str!("../../../examples/settop.toml")).unwrap();
        layers.save(&toml).unwrap();
        let text = std::fs::read_to_string(layers.editor()).unwrap();
        // A bad binding names it and leaves the file alone
        let pad = toml.profile.get_mut("pad").unwrap();
        pad.buttons.insert("a".into(), "stick".into());
        let err = layers.save(&toml).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile pad: buttons.a: Unknown target stick"
        );
        // So does a config the daemon could not start with
        let mut toml = ConfigToml::parse(include_str!("../../../examples/settop.toml")).unwrap();
        toml.daemon.active = Some("gone".into());
        assert!(layers.save(&toml).is_err());
        assert_eq!(std::fs::read_to_string(layers.editor()).unwrap(), text);
        // Nothing left behind
        assert_eq!(std::fs::read_dir(&layers.dir).unwrap().count(), 1);
        std::fs::remove_dir_all(root(&layers)).unwrap();
    }

    #[test]
    fn test_example_layers() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let layers = Layers {
            baseline: Some(examples.join("settop.toml")),
            dir: examples.join("config.d"),
        };
        let (toml, _) = layers.load().unwrap();
        let desktop = &toml.profile["desktop"].buttons;
        assert_eq!(desktop["x"], "mouse:middle");
        // the baseline's own binding survives the drop-in
        assert_eq!(desktop["a"], "key:KEY_ENTER");
        assert!(toml.profile.contains_key("media"));
    }

    #[test]
    fn test_bad_dropin_named() {
        let layers = layers("bad");
        std::fs::create_dir_all(&layers.dir).unwrap();
        std::fs::write(layers.dir.join("20-b.toml"), "[profile").unwrap();
        std::fs::write(layers.dir.join("10-a.toml"), BASE).unwrap();
        std::fs::write(layers.dir.join("notes.txt"), "[profile").unwrap();
        let err = layers.load().unwrap_err().to_string();
        assert!(
            err.starts_with(&format!("{}:", layers.dir.join("20-b.toml").display())),
            "{err}"
        );
        std::fs::remove_file(layers.dir.join("20-b.toml")).unwrap();
        assert_eq!(layers.load().unwrap().0.profile.len(), 1);
        std::fs::remove_dir_all(root(&layers)).unwrap();
    }

    #[test]
    fn test_save_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let layers = layers("ro");
        std::fs::create_dir_all(&layers.dir).unwrap();
        std::fs::set_permissions(&layers.dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = layers.save(&ConfigToml::default()).unwrap_err();
        std::fs::set_permissions(&layers.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            err.to_string(),
            format!("config is read-only: {}", layers.dir.display())
        );
        std::fs::remove_dir_all(root(&layers)).unwrap();
    }

    #[test]
    fn test_save_tuning() {
        let layers = layers("tune");
        std::fs::write(layers.baseline.as_ref().unwrap(), BASE).unwrap();
        std::fs::create_dir_all(&layers.dir).unwrap();
        std::fs::write(
            layers.editor(),
            "# mine\n[profile.p.buttons]\nx = \"key:KEY_X\"\n",
        )
        .unwrap();
        let (_, config) = layers.load().unwrap();
        let mut tuned = crate::engine::tuning("p", &config.get("p").unwrap());
        tuned.mouse_speed = 2.5;
        tuned.deadzone_stick = 0.2;
        tuned.haptics_menu = 0.7;
        layers.save_tuning(std::slice::from_ref(&tuned)).unwrap();
        let text = std::fs::read_to_string(layers.editor()).unwrap();
        // Only the changed knobs, and the editor's own binding and comment stay
        assert_eq!(
            text,
            concat!(
                "# mine\n\n[profile.p.buttons]\nx = \"key:KEY_X\"\n\n",
                "[profile.p.deadzone]\nstick = 0.2\n\n",
                "[profile.p.haptics]\nmenu = 0.7\n\n",
                "[profile.p.mouse]\nspeed = 2.5\n",
            )
        );
        // A reload gives the tuned values back exactly
        let (_, config) = layers.load().unwrap();
        assert_eq!(crate::engine::tuning("p", &config.get("p").unwrap()), tuned);
        std::fs::remove_dir_all(root(&layers)).unwrap();
    }

    #[test]
    fn test_save_tuning_read_only() {
        use std::os::unix::fs::PermissionsExt;

        let layers = layers("tune-ro");
        std::fs::write(layers.baseline.as_ref().unwrap(), BASE).unwrap();
        std::fs::create_dir_all(&layers.dir).unwrap();
        std::fs::set_permissions(&layers.dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let (_, config) = layers.load().unwrap();
        let tuned = crate::engine::tuning("p", &config.get("p").unwrap());
        let err = layers.save_tuning(&[tuned]).unwrap_err();
        std::fs::set_permissions(&layers.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(err.to_string().starts_with("config is read-only"), "{err}");
        std::fs::remove_dir_all(root(&layers)).unwrap();
    }
}
