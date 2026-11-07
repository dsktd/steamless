//! The 3D view on the page: a Blitz widget that shows the scene's texture and hands it the
//! pointer. The scene runs on a thread of its own (`scene`), started when Blitz first has a
//! device; this end talks to it over channels.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use anyrender::PaintRef;
use anyrender::PaintScene;
use anyrender::RenderContext;
use anyrender::ResourceId;
use anyrender::Scene;
use blitz_dom::Widget;
use blitz_dom::node::ComputedStyles;
use blitz_traits::events::BlitzWheelDelta;
use blitz_traits::events::UiEvent;
use dioxus_native::CustomWidgetAttr;
use dioxus_native::DeviceHandle;
use log::warn;
use peniko::Fill;
use peniko::ImageBrush;
use peniko::ImageSampler;
use peniko::kurbo::Affine;
use peniko::kurbo::Rect;
use tokio::sync::mpsc::UnboundedSender;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;

use crate::scene;

/// What the page asks of the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    /// The widget's size in physical pixels.
    Resize(u32, u32),
    /// A click at this point of the texture, 0 to 1 each way.
    Click(f32, f32),
    /// Pointer travel in CSS px while pressed.
    Drag(f32, f32),
    /// Scroll lines, positive toward the model.
    Zoom(f32),
    /// The controller's latest state.
    State(ControllerState),
    /// The page picked a control itself.
    Select(Option<Button>),
}

/// The page's end of the requests. The controller state is kept latest-only, as the scene may
/// sleep through many of them; the rest queue in order.
#[derive(Clone)]
pub struct Requests {
    queue: Sender<Request>,
    state: Arc<Mutex<Option<ControllerState>>>,
}
impl Requests {
    pub fn send(
        &self,
        request: Request,
    ) {
        match request {
            Request::State(state) => *self.state.lock().unwrap() = Some(state),
            // The scene is gone only when the window is
            request => {
                let _ = self.queue.send(request);
            }
        }
    }
}

/// The scene's end of [`Requests`].
pub struct Pending {
    queue: Receiver<Request>,
    state: Arc<Mutex<Option<ControllerState>>>,
}
impl Pending {
    /// Every request since the last take, the latest state first.
    pub fn take(&self) -> Vec<Request> {
        let state = self.state.lock().unwrap().take();
        state
            .map(Request::State)
            .into_iter()
            .chain(self.queue.try_iter())
            .collect()
    }
}

fn requests() -> (Requests, Pending) {
    let (queue, rx_queue) = mpsc::channel();
    let state = Arc::new(Mutex::new(None));
    let pending = Pending {
        queue: rx_queue,
        state: state.clone(),
    };
    (Requests { queue, state }, pending)
}

/// The page's end of the scene: the widget to put in the page and the way to talk to it.
#[derive(Clone)]
pub struct Model {
    pub widget: CustomWidgetAttr,
    pub requests: Requests,
}
impl Model {
    /// The scene reports every control picked on the model to `picks`.
    pub fn new(picks: UnboundedSender<Option<Button>>) -> Self {
        let view = View::new(picks);
        let requests = view.requests.clone();
        Self {
            widget: CustomWidgetAttr::new(view),
            requests,
        }
    }

    pub fn send(
        &self,
        request: Request,
    ) {
        self.requests.send(request);
    }
}

/// Pointer travel, in pixels, past which a press is a drag and not a click.
pub const CLICK_SLOP: f32 = 4.0;

/// A press on the view: a drag turns the model, a short one is a click.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Press {
    down: bool,
    last: (f32, f32),
    moved: f32,
}
impl Press {
    pub fn down(
        &mut self,
        x: f32,
        y: f32,
    ) {
        *self = Self {
            down: true,
            last: (x, y),
            moved: 0.0,
        };
    }

    /// The travel since the last move while pressed.
    pub fn moved_to(
        &mut self,
        x: f32,
        y: f32,
    ) -> Option<(f32, f32)> {
        if !self.down {
            return None;
        }
        let (dx, dy) = (x - self.last.0, y - self.last.1);
        self.last = (x, y);
        self.moved += (dx * dx + dy * dy).sqrt();
        Some((dx, dy))
    }

    /// The release: true for a click, false after a drag or with no press.
    pub fn up(&mut self) -> bool {
        let click = self.down && self.moved <= CLICK_SLOP;
        self.down = false;
        click
    }

    pub fn cancel(&mut self) {
        self.down = false;
    }
}

/// Element coordinates (CSS px) on a texture of `size` physical pixels at `scale`, 0 to 1
/// each way.
pub fn unit(
    size: (u32, u32),
    scale: f64,
    x: f32,
    y: f32,
) -> (f32, f32) {
    let (w, h) = size;
    (
        x * scale as f32 / w.max(1) as f32,
        y * scale as f32 / h.max(1) as f32,
    )
}

/// The largest side of the scene's target, in physical pixels.
pub const MAX_SIDE: u32 = 4096;

/// The target size for a paint of `width` by `height` physical pixels: none for an empty box,
/// each side clamped to [`MAX_SIDE`] so a runaway layout cannot ask for a huge texture.
pub fn target_size(
    width: u32,
    height: u32,
) -> Option<(u32, u32)> {
    (width > 0 && height > 0).then(|| (width.min(MAX_SIDE), height.min(MAX_SIDE)))
}

/// Scroll lines from a wheel delta; a pixel wheel counts 40 px a line.
pub fn lines(delta: &BlitzWheelDelta) -> f32 {
    match *delta {
        BlitzWheelDelta::Lines(_, y) => y as f32,
        BlitzWheelDelta::Pixels(_, y) => y as f32 / 40.0,
    }
}

/// `just ui-smoke-model`: with `YAKIN_SMOKE_RESIZE` set, the scene is asked for a few target
/// sizes in turn (one past [`MAX_SIDE`]) and kept drawing through each, then the process exits 0.
mod smoke {
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use super::{Request, Requests, target_size};

    const SIZES: [(u32, u32); 6] = [
        (1432, 763),
        (1432, 2552),
        (320, 200),
        (5000, 3000),
        (800, 600),
        (1432, 763),
    ];

    pub fn drive(
        requests: Requests,
        paints: Sender<()>,
    ) {
        if std::env::var_os("YAKIN_SMOKE_RESIZE").is_none() {
            return;
        }
        std::thread::spawn(move || {
            for (width, height) in SIZES {
                let (width, height) = target_size(width, height).unwrap();
                requests.send(Request::Resize(width, height));
                for _ in 0..60 {
                    let _ = paints.send(());
                    std::thread::sleep(Duration::from_millis(16));
                }
            }
            std::process::exit(0);
        });
    }
}

struct View {
    requests: Requests,
    /// The scene's ends, kept until the scene thread takes them in `can_create_surfaces`.
    pending: Option<scene::Ends>,
    textures: Receiver<wgpu::TextureView>,
    /// Keeps the scene drawing: it sleeps when the paints stop.
    paints: Sender<()>,
    shown: Option<((u32, u32), ResourceId)>,
    /// Physical pixels of the last paint.
    size: (u32, u32),
    scale: f64,
    press: Press,
}
impl View {
    fn new(picks: UnboundedSender<Option<Button>>) -> Self {
        let (requests, pending) = requests();
        let (tx_textures, textures) = mpsc::channel();
        let (paints, rx_paints) = mpsc::channel();
        Self {
            requests,
            pending: Some(scene::Ends {
                requests: pending,
                textures: tx_textures,
                picks,
                paints: rx_paints,
            }),
            textures,
            paints,
            shown: None,
            size: (0, 0),
            scale: 1.0,
            press: Press::default(),
        }
    }
}

impl Widget for View {
    fn can_create_surfaces(
        &mut self,
        render_ctx: &mut dyn RenderContext,
    ) {
        let Some(ctx) = render_ctx.renderer_specific_context() else {
            warn!("the renderer gave no context: no 3D view");
            return;
        };
        let Ok(handle) = ctx.downcast::<DeviceHandle>() else {
            warn!("not a wgpu renderer: no 3D view");
            return;
        };
        // A resume after a suspend: the scene thread is still running
        let Some(ends) = self.pending.take() else {
            return;
        };
        smoke::drive(self.requests.clone(), self.paints.clone());
        std::thread::spawn(move || scene::run(*handle, ends));
    }

    fn destroy_surfaces(&mut self) {
        self.shown = None;
    }

    fn requires_redraw(&self) -> bool {
        true
    }

    fn handle_event(
        &mut self,
        event: &UiEvent,
    ) {
        match event {
            UiEvent::PointerDown(p) => self.press.down(p.element.x, p.element.y),
            UiEvent::PointerMove(p) => {
                if let Some((dx, dy)) = self.press.moved_to(p.element.x, p.element.y) {
                    self.requests.send(Request::Drag(dx, dy));
                }
            }
            UiEvent::PointerUp(p) => {
                if self.press.up() {
                    let (x, y) = unit(self.size, self.scale, p.element.x, p.element.y);
                    self.requests.send(Request::Click(x, y));
                }
            }
            UiEvent::PointerCancel(_) | UiEvent::PointerLeave(_) => self.press.cancel(),
            UiEvent::Wheel(w) => {
                self.requests.send(Request::Zoom(-lines(&w.delta)));
            }
            _ => {}
        }
    }

    fn paint(
        &mut self,
        render_ctx: &mut dyn RenderContext,
        _styles: &ComputedStyles,
        width: u32,
        height: u32,
        scale: f64,
    ) -> Scene {
        self.scale = scale;
        let _ = self.paints.send(());
        if let Some(size) = target_size(width, height)
            && self.size != size
        {
            self.size = size;
            self.requests.send(Request::Resize(size.0, size.1));
        }
        // The scene made a new target: show the latest one
        while let Ok(view) = self.textures.try_recv() {
            if let Some((_, old)) = self.shown.take() {
                render_ctx.unregister_resource(old);
            }
            let size = (view.texture().width(), view.texture().height());
            match render_ctx.try_register_custom_resource(Box::new(view)) {
                Ok(id) => self.shown = Some((size, id)),
                Err(err) => warn!("register the scene's texture: {err:?}"),
            }
        }
        let mut scene = Scene::new();
        if let Some(((w, h), id)) = &self.shown {
            // Drawn at the texture's size: a stale one is cropped or padded until the resize lands
            let rect = Rect::from_origin_size((0.0, 0.0), (*w as f64, *h as f64));
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                PaintRef::Resource(ImageBrush {
                    image: *id,
                    sampler: ImageSampler::default(),
                }),
                None,
                &rect,
            );
        }
        scene
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_target_size_skips_empty_and_clamps() {
        assert_eq!(target_size(0, 480), None);
        assert_eq!(target_size(640, 0), None);
        assert_eq!(target_size(1432, 763), Some((1432, 763)));
        assert_eq!(target_size(9000, 20000), Some((MAX_SIDE, MAX_SIDE)));
    }

    #[test]
    fn test_requests_keep_the_latest_state() {
        let (requests, pending) = requests();
        let mut state = ControllerState::default();
        for seq in 0..100 {
            state.seq = seq;
            requests.send(Request::State(state));
        }
        requests.send(Request::Zoom(1.0));
        requests.send(Request::Drag(1.0, 2.0));
        assert_eq!(
            pending.take(),
            vec![
                Request::State(state),
                Request::Zoom(1.0),
                Request::Drag(1.0, 2.0)
            ]
        );
        assert_eq!(pending.take(), vec![]);
    }

    #[test]
    fn test_press_click() {
        let mut press = Press::default();
        assert!(!press.up(), "no press, no click");
        press.down(10.0, 10.0);
        assert_eq!(press.moved_to(12.0, 10.0), Some((2.0, 0.0)));
        assert_eq!(press.moved_to(12.0, 11.0), Some((0.0, 1.0)));
        assert!(press.up(), "3 px of travel is a click");
        assert_eq!(press.moved_to(20.0, 20.0), None);
    }

    #[test]
    fn test_press_drag() {
        let mut press = Press::default();
        press.down(0.0, 0.0);
        press.moved_to(3.0, 0.0);
        press.moved_to(0.0, 0.0);
        assert!(!press.up(), "6 px there and back is a drag");
        press.down(0.0, 0.0);
        press.cancel();
        assert!(!press.up());
    }

    #[test]
    fn test_unit() {
        assert_eq!(unit((200, 100), 1.0, 50.0, 100.0), (0.25, 1.0));
        // At scale 2 a CSS px is two texture px
        assert_eq!(unit((200, 100), 2.0, 50.0, 25.0), (0.5, 0.5));
        assert_eq!(unit((0, 0), 1.0, 3.0, 4.0), (3.0, 4.0));
    }

    #[test]
    fn test_lines() {
        assert_eq!(lines(&BlitzWheelDelta::Lines(0.0, 2.0)), 2.0);
        assert_eq!(lines(&BlitzWheelDelta::Pixels(0.0, -80.0)), -2.0);
    }
}
