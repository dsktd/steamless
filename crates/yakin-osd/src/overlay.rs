//! The layer-shell surfaces. The toast's exists only while a toast is up: created on show,
//! destroyed once the fade ends, so an idle OSD has nothing mapped. The menu's covers the screen
//! while the daemon has the menu open, the keyboard's is docked at the bottom while it has the
//! keyboard open; a toast still shows on its own surface above them.

use std::time::Duration;
use std::time::Instant;

use log::warn;
use resvg::tiny_skia::Pixmap;
use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::compositor::FrameCallbackData;
use smithay_client_toolkit::compositor::Region;
use smithay_client_toolkit::delegate_dispatch2;
use smithay_client_toolkit::delegate_registry;
use smithay_client_toolkit::output::OutputHandler;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::reexports::calloop::LoopHandle;
use smithay_client_toolkit::reexports::calloop::RegistrationToken;
use smithay_client_toolkit::reexports::calloop::timer::TimeoutAction;
use smithay_client_toolkit::reexports::calloop::timer::Timer;
use smithay_client_toolkit::reexports::client::Connection;
use smithay_client_toolkit::reexports::client::QueueHandle;
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::wl_output;
use smithay_client_toolkit::reexports::client::protocol::wl_shm;
use smithay_client_toolkit::reexports::client::protocol::wl_surface;
use smithay_client_toolkit::registry::ProvidesRegistryState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use smithay_client_toolkit::shell::wlr_layer::LayerShell;
use smithay_client_toolkit::shell::wlr_layer::LayerShellHandler;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure;
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::ShmHandler;
use smithay_client_toolkit::shm::slot::SlotPool;
use yakin_ipc::IpcKeyboard;
use yakin_ipc::IpcMenu;

use crate::draw;
use crate::draw::Painter;
use crate::keyboard::keyboard_height;
use crate::keyboard::keyboard_svg;
use crate::menu::menu_svg;
use crate::toast::Toast;

const FADE: Duration = Duration::from_millis(200);

/// Distance from the anchored edge, logical pixels.
const MARGIN: i32 = 48;

/// When the process is done.
#[derive(Clone, Copy, PartialEq)]
pub enum Exit {
    Never,
    /// After the toast has faded (`--show`).
    Faded,
    /// At the first frame callback (`--once`).
    Shown,
}

pub struct Overlay {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    qh: QueueHandle<Overlay>,
    handle: LoopHandle<'static, Overlay>,
    painter: Painter,
    anchor: Anchor,
    hold: Duration,
    exit: Exit,
    /// Set when the process should end, with how.
    pub status: Option<anyhow::Result<()>>,
    shown: Option<Shown>,
    menu: Option<Panel>,
    keyboard: Option<Panel>,
}

/// What a panel shows.
enum Content {
    Menu(IpcMenu),
    Keyboard(IpcKeyboard),
}
impl Content {
    fn svg(
        &self,
        size: (u32, u32),
    ) -> String {
        match self {
            Content::Menu(menu) => menu_svg(menu, size),
            Content::Keyboard(keyboard) => keyboard_svg(keyboard, size),
        }
    }
}

/// The open menu or keyboard and its surface.
struct Panel {
    content: Content,
    layer: LayerSurface,
    /// Logical size, from the first configure.
    size: Option<(u32, u32)>,
    scale: u32,
}

#[derive(Clone, Copy)]
enum Kind {
    Menu,
    Keyboard,
}

/// The toast on screen and its surface.
struct Shown {
    toast: Toast,
    layer: LayerSurface,
    configured: bool,
    scale: u32,
    pixmap: Option<Pixmap>,
    hold: Option<RegistrationToken>,
    fading: Option<Instant>,
}

impl Overlay {
    pub fn new(
        globals: &GlobalList,
        qh: QueueHandle<Overlay>,
        handle: LoopHandle<'static, Overlay>,
        anchor: Anchor,
        hold: Duration,
        exit: Exit,
    ) -> anyhow::Result<Self> {
        let shm = Shm::bind(globals, &qh)?;
        let pool = SlotPool::new((draw::WIDTH * draw::HEIGHT * 4) as usize, &shm)?;
        Ok(Self {
            registry: RegistryState::new(globals),
            outputs: OutputState::new(globals, &qh),
            compositor: CompositorState::bind(globals, &qh)?,
            layer_shell: LayerShell::bind(globals, &qh)?,
            shm,
            pool,
            qh,
            handle,
            painter: Painter::new(),
            anchor,
            hold,
            exit,
            status: None,
            shown: None,
            menu: None,
            keyboard: None,
        })
    }

    /// Show `toast` now, replacing whatever is up.
    pub fn show(
        &mut self,
        toast: Toast,
    ) {
        let hold = self
            .handle
            .insert_source(Timer::from_duration(self.hold), |_, _, overlay| {
                overlay.fade();
                TimeoutAction::Drop
            })
            .ok();
        match &mut self.shown {
            Some(shown) => {
                if let Some(old) = shown.hold.take() {
                    self.handle.remove(old);
                }
                shown.toast = toast;
                shown.pixmap = None;
                shown.fading = None;
                shown.hold = hold;
            }
            None => {
                self.shown = Some(Shown {
                    toast,
                    layer: self.layer(),
                    configured: false,
                    scale: self.scale(),
                    pixmap: None,
                    hold,
                    fading: None,
                })
            }
        }
        self.draw();
    }

    /// Show `menu`, or take it down.
    pub fn set_menu(
        &mut self,
        menu: Option<IpcMenu>,
    ) {
        let Some(menu) = menu else {
            self.menu = None;
            return;
        };
        match &mut self.menu {
            Some(shown) => shown.content = Content::Menu(menu),
            None => {
                self.menu = Some(Panel {
                    content: Content::Menu(menu),
                    layer: self.menu_layer(),
                    size: None,
                    scale: self.scale(),
                })
            }
        }
        self.draw_panel(Kind::Menu);
    }

    /// Show `keyboard`, or take it down.
    pub fn set_keyboard(
        &mut self,
        keyboard: Option<IpcKeyboard>,
    ) {
        let Some(keyboard) = keyboard else {
            self.keyboard = None;
            return;
        };
        match &mut self.keyboard {
            Some(shown) => shown.content = Content::Keyboard(keyboard),
            None => {
                self.keyboard = Some(Panel {
                    content: Content::Keyboard(keyboard),
                    layer: self.keyboard_layer(),
                    size: None,
                    scale: self.scale(),
                })
            }
        }
        self.draw_panel(Kind::Keyboard);
    }

    /// A full-width surface docked at the bottom, a share of the tallest output high, that
    /// takes no input: the daemon drives the keyboard from the controller.
    fn keyboard_layer(&self) -> LayerSurface {
        let height = self
            .outputs
            .outputs()
            .filter_map(|output| self.outputs.info(&output))
            .filter_map(|info| info.logical_size)
            .map(|(_, height)| height.max(1) as u32)
            .max()
            .unwrap_or(540);
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("yakin-keyboard"),
            None,
        );
        layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, keyboard_height(height));
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(0);
        match Region::new(&self.compositor) {
            Ok(region) => layer.set_input_region(Some(region.wl_region())),
            Err(err) => warn!("input region: {err}"),
        }
        layer.commit();
        layer
    }

    /// A surface over the whole output, panels included, that takes no input: the daemon
    /// drives the menu from the controller.
    fn menu_layer(&self) -> LayerSurface {
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("yakin-menu"),
            None,
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(-1);
        match Region::new(&self.compositor) {
            Ok(region) => layer.set_input_region(Some(region.wl_region())),
            Err(err) => warn!("input region: {err}"),
        }
        layer.commit();
        layer
    }

    fn draw_panel(
        &mut self,
        kind: Kind,
    ) {
        let shown = match kind {
            Kind::Menu => &self.menu,
            Kind::Keyboard => &self.keyboard,
        };
        let Some(shown) = shown else {
            return;
        };
        let Some(size) = shown.size else {
            return;
        };
        let svg = shown.content.svg(size);
        let pixmap = match self.painter.render_svg(&svg, size, shown.scale) {
            Ok(pixmap) => pixmap,
            Err(err) => return self.panel_fail(kind, err),
        };
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let (buffer, canvas) =
            match self
                .pool
                .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
            {
                Ok(created) => created,
                Err(err) => return self.panel_fail(kind, err.into()),
            };
        draw::copy(&pixmap, canvas, 1.0);
        let surface = shown.layer.wl_surface();
        surface.set_buffer_scale(shown.scale as i32);
        surface.damage_buffer(0, 0, width, height);
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        if let Err(err) = buffer.attach_to(surface) {
            return self.panel_fail(kind, err.into());
        }
        shown.layer.commit();
    }

    /// Drop the panel; a one-shot run ends with `err`, the follower only logs it.
    fn panel_fail(
        &mut self,
        kind: Kind,
        err: anyhow::Error,
    ) {
        match kind {
            Kind::Menu => self.menu = None,
            Kind::Keyboard => self.keyboard = None,
        }
        match self.exit {
            Exit::Never => warn!("osd panel: {err}"),
            _ => self.status = Some(Err(err)),
        }
    }

    /// Which panel `surface` belongs to, if any.
    fn panel_of(
        &self,
        surface: &wl_surface::WlSurface,
    ) -> Option<Kind> {
        let owns = |panel: &Option<Panel>| {
            panel
                .as_ref()
                .is_some_and(|shown| shown.layer.wl_surface() == surface)
        };
        match (owns(&self.menu), owns(&self.keyboard)) {
            (true, _) => Some(Kind::Menu),
            (_, true) => Some(Kind::Keyboard),
            _ => None,
        }
    }

    fn panel_mut(
        &mut self,
        kind: Kind,
    ) -> &mut Option<Panel> {
        match kind {
            Kind::Menu => &mut self.menu,
            Kind::Keyboard => &mut self.keyboard,
        }
    }

    /// A new overlay surface, committed without a buffer to ask for its first configure.
    fn layer(&self) -> LayerSurface {
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Overlay,
            Some("yakin-osd"),
            None,
        );
        layer.set_anchor(self.anchor);
        layer.set_margin(MARGIN, 0, MARGIN, 0);
        layer.set_size(draw::WIDTH, draw::HEIGHT);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(0);
        // An empty input region: clicks fall through to whatever is below
        match Region::new(&self.compositor) {
            Ok(region) => layer.set_input_region(Some(region.wl_region())),
            Err(err) => warn!("input region: {err}"),
        }
        layer.commit();
        layer
    }

    /// The largest output scale: the guess until the surface reports the one it is on.
    fn scale(&self) -> u32 {
        self.outputs
            .outputs()
            .filter_map(|output| self.outputs.info(&output))
            .map(|info| info.scale_factor.max(1) as u32)
            .max()
            .unwrap_or(1)
    }

    fn fade(&mut self) {
        if let Some(shown) = &mut self.shown {
            shown.hold = None;
            shown.fading = Some(Instant::now());
        }
        self.draw();
    }

    /// Commit the current frame, asking for a frame callback; past the fade, drop the surface.
    fn draw(&mut self) {
        let Some(shown) = &mut self.shown else {
            return;
        };
        if !shown.configured {
            return;
        }
        let alpha = match shown.fading {
            Some(start) => 1.0 - start.elapsed().as_secs_f32() / FADE.as_secs_f32(),
            None => 1.0,
        };
        if alpha <= 0.0 {
            self.shown = None;
            if self.exit == Exit::Faded {
                self.status = Some(Ok(()));
            }
            return;
        }
        if shown.pixmap.is_none() {
            match self.painter.render(&shown.toast, shown.scale) {
                Ok(pixmap) => shown.pixmap = Some(pixmap),
                Err(err) => return self.fail(err),
            }
        }
        let Some(pixmap) = &shown.pixmap else {
            return;
        };
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let (buffer, canvas) =
            match self
                .pool
                .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
            {
                Ok(created) => created,
                Err(err) => return self.fail(err.into()),
            };
        draw::copy(pixmap, canvas, alpha);
        let surface = shown.layer.wl_surface();
        surface.set_buffer_scale(shown.scale as i32);
        surface.damage_buffer(0, 0, width, height);
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        if let Err(err) = buffer.attach_to(surface) {
            return self.fail(err.into());
        }
        shown.layer.commit();
    }

    /// Drop the toast; a one-shot run ends with `err`, the follower only logs it.
    fn fail(
        &mut self,
        err: anyhow::Error,
    ) {
        self.shown = None;
        match self.exit {
            Exit::Never => warn!("osd: {err}"),
            _ => self.status = Some(Err(err)),
        }
    }
}

impl CompositorHandler for Overlay {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let scale = new_factor.max(1) as u32;
        if let Some(kind) = self.panel_of(surface) {
            if let Some(shown) = self.panel_mut(kind)
                && shown.scale != scale
            {
                shown.scale = scale;
                self.draw_panel(kind);
            }
            return;
        }
        if let Some(shown) = &mut self.shown
            && shown.scale != scale
        {
            shown.scale = scale;
            shown.pixmap = None;
            self.draw();
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if self.exit == Exit::Shown {
            eprintln!("osd: shown");
            self.status = Some(Ok(()));
            return;
        }
        if self.panel_of(surface).is_none()
            && self
                .shown
                .as_ref()
                .is_some_and(|shown| shown.fading.is_some())
        {
            self.draw();
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for Overlay {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
    ) {
        if let Some(kind) = self.panel_of(layer.wl_surface()) {
            *self.panel_mut(kind) = None;
            return;
        }
        if let Some(shown) = self.shown.take()
            && let Some(hold) = shown.hold
        {
            self.handle.remove(hold);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(kind) = self.panel_of(layer.wl_surface()) {
            if let Some(shown) = self.panel_mut(kind) {
                let (width, height) = configure.new_size;
                shown.size = (width > 0 && height > 0).then_some((width, height));
            }
            self.draw_panel(kind);
            return;
        }
        if let Some(shown) = &mut self.shown {
            shown.configured = true;
        }
        self.draw();
    }
}

impl ShmHandler for Overlay {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState];
}

delegate_registry!(Overlay);
delegate_dispatch2!(Overlay);
