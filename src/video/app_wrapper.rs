//! A windowing runner that drives egui on an EGL OpenGL context and supports
//! egui's **immediate viewports** as real OS windows.
//!
//! Two things make this different from the stock `egui_glow` example:
//!
//! 1. It forces an EGL context (`ApiPreference::PreferEgl`) on every platform,
//!    including X11, so the DMA-BUF import works everywhere. The consumer writes
//!    no EGL code.
//! 2. It implements multi-window support for `egui::Context::show_viewport_immediate`.
//!    Upstream `egui_glow::EguiGlow` only renders the root viewport and warns on
//!    others; here each immediate viewport gets its own native window, surface,
//!    and input state, so you can pop the PipeWire video (plus any other egui
//!    widgets) out into a separate window. Input and window-close are handled.
//!
//! Only *immediate* viewports are supported (not deferred ones), per the
//! requirement. The structure mirrors eframe's glow integration: one shared
//! `egui::Context`, one `egui_glow::Painter` and one GL context (made current on
//! each window's surface in turn), a per-viewport map of window/surface/winit
//! state, an immediate-viewport renderer registered on the context, and a
//! thread-local holding the active event loop so that renderer can create
//! windows.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eframe::{egui, egui_glow, glow};

use egui::{
    ImmediateViewport, OrderedViewportIdMap, ViewportBuilder, ViewportId, ViewportInfo,
    ViewportOutput,
};


use egui_winit::winit;
use egui_winit::winit::window::Fullscreen;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::raw_window_handle::HasWindowHandle as _;
use winit::window::{Window, WindowId};

use glutin::config::{Config, ConfigTemplateBuilder};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext as _, PossiblyCurrentContext,
    PossiblyCurrentGlContext as _,
};
use glutin::display::{Display, GetGlDisplay as _, GlDisplay as _};
use glutin::surface::{
    GlSurface as _, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface,
};
use glutin_winit::{ApiPreference, DisplayBuilder};

/// Boxed error type returned by [`run`] and the app factory.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Wrap an error with context, returning a [`BoxError`].
macro_rules! ctx {
    ($result:expr, $msg:expr) => {
        $result.map_err(|e| -> BoxError { format!("{}: {e}", $msg).into() })
    };
}

/// Handed to your app factory once the GL context exists. Build your
/// [`crate::PipewireVideo`] widgets from this.

#[derive(Clone)]
pub struct CreationContext {
    /// The live OpenGL context (an EGL context).
    pub gl: std::sync::Arc<glow::Context>,
    /// The shared egui context (used for all viewports).
    pub egui_ctx: egui::Context,
    pub glstate: Rc<RefCell<GlState>>,
}

/// Your application. Implement [`ui`](App::ui) for the main window; inside it
/// you may call `ctx.show_viewport_immediate(..)` to render into extra OS
/// windows. Optionally implement [`on_exit`](App::on_exit) to release GPU
/// resources while the context is still current.
pub trait App {
    /// Build the main window's UI.
    fn ui(&mut self, ui: &mut egui::Ui);
    /// Called once at shutdown, with the GL context still current.
    fn on_exit(&mut self, _gl: &glow::Context) {}
}

type Factory = Box<dyn FnOnce(&CreationContext) -> Result<Box<dyn App>, BoxError>>;

#[derive(Debug)]
enum UserEvent {
    /// egui asked for a repaint (possibly from the PipeWire thread); wake up.
    Redraw(Duration),
}

/// Stores the active event loop in a thread-local for the duration of each
/// `ApplicationHandler` callback, so the immediate-viewport renderer (which
/// cannot capture the event loop) can create windows. Lifted from eframe.
mod event_loop_context {
    use std::cell::Cell;
    use super::winit::event_loop::ActiveEventLoop;

    thread_local! {
        static CURRENT_EVENT_LOOP: Cell<Option<*const ActiveEventLoop>> = const { Cell::new(None) };
    }

    struct Guard;

    impl Guard {
        fn new(event_loop: &ActiveEventLoop) -> Self {
            CURRENT_EVENT_LOOP.with(|cell| {
                assert!(cell.get().is_none(), "event loop already set");
                cell.set(Some(std::ptr::from_ref::<ActiveEventLoop>(event_loop)));
            });
            Self
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            CURRENT_EVENT_LOOP.with(|cell| cell.set(None));
        }
    }

    /// Run `f` with the current event loop available to [`with_current_event_loop`].
    pub fn with_event_loop_context(event_loop: &ActiveEventLoop, f: impl FnOnce()) {
        let _guard = Guard::new(event_loop);
        f();
    }

    /// Access the event loop set by [`with_event_loop_context`], if any.
    pub fn with_current_event_loop<F, R>(f: F) -> Option<R>
    where
        F: FnOnce(&ActiveEventLoop) -> R,
    {
        CURRENT_EVENT_LOOP.with(|cell| {
            cell.get().map(|ptr| {
                // SAFETY: the pointer is valid while Some: the Guard that set it
                // lives at least as long as this call and clears it on drop, and
                // it was made from a shared borrow so no &mut exists.
                let event_loop = unsafe { &*ptr };
                f(event_loop)
            })
        })
    }
}

/// One viewport: its identity, the latest builder/info, and (once created) the
/// native window, GL surface, and winit input state. The last three are created
/// together the first time the viewport is shown.
struct Viewport {
    parent: Option<ViewportId>,
    builder: ViewportBuilder,
    info: ViewportInfo,
    window: Option<Window>,
    surface: Option<Surface<WindowSurface>>,
    egui_winit: Option<egui_winit::State>,
}

/// The data the painter needs for one viewport's frame.
struct FramePaint<'a> {
    clipped: &'a [egui::ClippedPrimitive],
    textures_delta: &'a egui::TexturesDelta,
    pixels_per_point: f32,
}

/// Create a viewport's window, GL surface, winit state, and info in one go.
fn create_viewport(
    egui_ctx: &egui::Context,
    display: &Display,
    gl_config: &Config,
    window_attributes: winit::window::WindowAttributes,
    context: &PossiblyCurrentContext,
    builder: &ViewportBuilder,
    vid: ViewportId,
    max_texture_side: Option<usize>,
    event_loop: &ActiveEventLoop,
) -> Result<(Window, Surface<WindowSurface>, egui_winit::State, ViewportInfo), BoxError> {
    let window =
        ctx!(glutin_winit::finalize_window(event_loop, window_attributes, gl_config), "create window")?;

    egui_winit::apply_viewport_builder_to_window(egui_ctx, &window, builder);
    let mut info = ViewportInfo::default();
    egui_winit::update_viewport_info(&mut info, egui_ctx, &window, true);

    let surface = ctx!(create_surface(display, gl_config, &window), "create surface")?;
    ctx!(context.make_current(&surface), "make current")?;
    let _ = surface.set_swap_interval(context, SwapInterval::Wait(NonZeroU32::MIN));

    let state = egui_winit::State::new(
        egui_ctx.clone(),
        vid,
        event_loop,
        Some(window.scale_factor() as f32),
        event_loop.system_theme(),
        max_texture_side,
    );

    Ok((window, surface, state, info))
}

/// Create a GL window surface for a given window.
fn create_surface(
    display: &Display,
    gl_config: &Config,
    window: &Window,
) -> Result<Surface<WindowSurface>, BoxError> {
    let (w, h): (u32, u32) = window.inner_size().into();
    let (w, h) = (
        NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN),
        NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN),
    );
    let surface_attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(
        ctx!(window.window_handle(), "window handle")?.as_raw(),
        w,
        h,
    );
    let surface = unsafe {
        ctx!(display.create_window_surface(gl_config, &surface_attributes), "create surface")?
    };
    Ok(surface)
}

/// Paint one viewport: gather input, run UI, paint to surface, process outputs.
fn paint_viewport<F: FnMut(&mut egui::Ui)>(
    glstate: &Rc<RefCell<GlState>>,
    painter: &Rc<RefCell<egui_glow::Painter>>,
    egui_ctx: &egui::Context,
    vid: ViewportId,
    prune: bool,
    ui_cb: F,
    beginning: Instant,
) {
    // 1. Gather input.
    let mut raw_input = {
        let mut gs = glstate.borrow_mut();
        let Some(vp) = gs.viewports.get_mut(&vid) else {
            return;
        };
        let (Some(window), Some(state)) =
            (vp.window.as_ref(), vp.egui_winit.as_mut())
        else {
            return;
        };
        egui_winit::update_viewport_info(&mut vp.info, egui_ctx, window, false);
        let mut ri = state.take_egui_input(window);
        ri.time = Some(beginning.elapsed().as_secs_f64());
        ri
    };
    {
        let gs = glstate.borrow();
        raw_input.viewports = gs
            .viewports
            .iter()
            .map(|(id, v)| (*id, v.info.clone()))
            .collect();
    }

    // 2. Run UI.
    let full = egui_ctx.run_ui(raw_input, ui_cb);

    // 3. Paint.
    let clipped = egui_ctx.tessellate(full.shapes, full.pixels_per_point);
    {
        let mut gs = glstate.borrow_mut();
        let mut p = painter.borrow_mut();
        gs.present_viewport(
            vid,
            &mut p,
            FramePaint {
                clipped: &clipped,
                textures_delta: &full.textures_delta,
                pixels_per_point: full.pixels_per_point,
            },
            full.platform_output,
        );
        if let Some(vp) = gs.viewports.get_mut(&vid) {
            vp.info.events.clear();
        }
    }

    // 4. Process outputs.
    {
        let mut gs = glstate.borrow_mut();
        gs.process_viewport_outputs(egui_ctx, &full.viewport_output, prune);
    }
}

/// Shared GL + windowing state. One GL context is made current on each
/// viewport's surface in turn; one `egui_glow::Painter` (held separately) draws
/// to whichever surface is current.
pub struct GlState {
    display: Display,
    gl_config: Config,
    context: PossiblyCurrentContext,
    max_texture_side: Option<usize>,
    egui_ctx: egui::Context,
    viewports: HashMap<ViewportId, Viewport>,
    viewport_from_window: HashMap<WindowId, ViewportId>,
}

impl GlState {
    /// Gets the monitors avaliable for root window.
    pub fn get_monitors(&mut self) -> Option<Vec<winit::monitor::MonitorHandle>>{
        self.viewports.get(&ViewportId::ROOT).and_then(|viewport| viewport.window.as_ref().and_then(|window| Some(window.available_monitors().collect())))
    }
    /// Request window be fullscreened on a certain monitor.
    pub fn request_fullscreen(&mut self, id: &ViewportId, fullscreen: Option<Fullscreen>) {
        let Some(viewport) = self.viewports.get_mut(id) else {return};
        let Some(window) = &viewport.window else {return};
        window.set_fullscreen(fullscreen);
    }

    /// Insert or update a viewport entry. Drops window/surface/state if builder
    /// changed in a way that requires recreation.
    fn upsert_viewport(&mut self, vid: ViewportId, parent: Option<ViewportId>, builder: ViewportBuilder) {
        use std::collections::hash_map::Entry;
        match self.viewports.entry(vid) {
            Entry::Vacant(entry) => {
                entry.insert(Viewport {
                    parent,
                    builder,
                    info: ViewportInfo::default(),
                    window: None,
                    surface: None,
                    egui_winit: None,
                });
            }
            Entry::Occupied(mut entry) => {
                let vp = entry.get_mut();
                vp.parent = parent;
                let (_delta, recreate) = vp.builder.patch(builder);
                if recreate {
                    vp.window = None;
                    vp.surface = None;
                    vp.egui_winit = None;
                }
            }
        }
    }

    /// Create the window, surface, and input state for a viewport if missing.
    fn ensure_window(&mut self, vid: ViewportId, event_loop: &ActiveEventLoop) -> Result<(), BoxError> {
        let GlState {
            display,
            gl_config,
            context,
            max_texture_side,
            egui_ctx,
            viewports,
            viewport_from_window,
        } = self;

        let Some(vp) = viewports.get_mut(&vid) else {
            return Err("viewport entry missing".into());
        };
        if vp.window.is_some() {
            return Ok(());
        }

        let attrs = egui_winit::create_winit_window_attributes(egui_ctx, vp.builder.clone());
        let (window, surface, state, info) = create_viewport(
            egui_ctx, display, gl_config, attrs, context, &vp.builder,
            vid, *max_texture_side, event_loop,
        )?;

        let id = window.id();
        vp.window = Some(window);
        vp.surface = Some(surface);
        vp.egui_winit = Some(state);
        vp.info = info;
        viewport_from_window.insert(id, vid);
        Ok(())
    }

    fn resize(&mut self, vid: ViewportId, size: PhysicalSize<u32>) {
        let GlState { context, viewports, .. } = self;
        if let Some(vp) = viewports.get_mut(&vid) {
            if let Some(surface) = vp.surface.as_ref() {
                let w = NonZeroU32::new(size.width.max(1)).unwrap_or(NonZeroU32::MIN);
                let h = NonZeroU32::new(size.height.max(1)).unwrap_or(NonZeroU32::MIN);
                surface.resize(context, w, h);
            }
        }
    }

    /// Make a viewport's surface current, clear, paint egui (this is where the
    /// video widget's paint callback runs and blits into the window), present,
    /// and dispatch platform output.
    fn present_viewport(
        &mut self,
        vid: ViewportId,
        painter: &mut egui_glow::Painter,
        frame: FramePaint<'_>,
        platform_output: egui::PlatformOutput,
    ) {
        let GlState { context, viewports, .. } = self;
        let Some(vp) = viewports.get_mut(&vid) else { return };
        let (Some(window), Some(surface), Some(state)) =
            (vp.window.as_ref(), vp.surface.as_ref(), vp.egui_winit.as_mut())
        else {
            return;
        };

        if context.make_current(surface).is_err() {
            return;
        }
        let size: [u32; 2] = window.inner_size().into();
        painter.clear(size, [0.0, 0.0, 0.0, 0.0]);
        painter.paint_and_update_textures(
            size,
            frame.pixels_per_point,
            frame.clipped,
            frame.textures_delta,
        );
        if let Err(e) = surface.swap_buffers(context) {
            eprintln!("egui-pw-dmabuf: swap_buffers failed: {e}");
        }
        state.handle_platform_output(window, platform_output);
    }

    /// Apply viewport commands to existing windows, and (when `prune`) drop any
    /// viewport not present in `viewport_output`. Only the root frame prunes,
    /// since a child frame's output does not list its parent/siblings.
    fn process_viewport_outputs(
        &mut self,
        egui_ctx: &egui::Context,
        viewport_output: &OrderedViewportIdMap<ViewportOutput>,
        prune: bool,
    ) {
        for (vid, out) in viewport_output {
            self.upsert_viewport(*vid, Some(out.parent), out.builder.clone());

            let GlState { viewports, .. } = self;
            if let Some(vp) = viewports.get_mut(vid) {
                if let Some(window) = vp.window.as_ref() {
                    let mut actions = Vec::new();
                    egui_winit::process_viewport_commands(
                        egui_ctx,
                        &mut vp.info,
                        out.commands.clone(),
                        window,
                        &mut actions,
                    );
                }
            }
        }

        if prune {
            self.viewports.retain(|id, _| viewport_output.contains_key(id));
            self.viewport_from_window
                .retain(|_, id| viewport_output.contains_key(id));
        }
    }
}

/// Open a window with an EGL OpenGL context and run `app`, supporting immediate
/// viewports as native windows.
pub fn run<F>(title: impl Into<String>, factory: F) -> Result<(), BoxError>
where
    F: FnOnce(&CreationContext) -> Result<Box<dyn App>, BoxError> + 'static,
{
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| Box::new(e) as BoxError)?;
    let proxy = event_loop.create_proxy();

    let mut runner = Runner {
        title: title.into(),
        proxy,
        factory: Some(Box::new(factory)),
        glstate: None,
        painter: None,
        gl: None,
        egui_ctx: egui::Context::default(),
        app: None,
        repaint_delay: Duration::MAX,
        setup_error: None,
        beginning: Instant::now(),
    };

    event_loop
        .run_app(&mut runner)
        .map_err(|e| Box::new(e) as BoxError)?;

    match runner.setup_error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct Runner {
    title: String,
    proxy: EventLoopProxy<UserEvent>,
    factory: Option<Factory>,
    glstate: Option<Rc<RefCell<GlState>>>,
    painter: Option<Rc<RefCell<egui_glow::Painter>>>,
    gl: Option<std::sync::Arc<glow::Context>>,
    egui_ctx: egui::Context,
    app: Option<Box<dyn App>>,
    repaint_delay: Duration,
    setup_error: Option<BoxError>,
    beginning: Instant,
}

impl Runner {
    fn request_root_redraw(&self) {
        if let Some(gs) = self.glstate.as_ref() {
            if let Some(vp) = gs.borrow().viewports.get(&ViewportId::ROOT) {
                if let Some(w) = &vp.window {
                    w.request_redraw();
                }
            }
        }
    }

    /// Render the root viewport. During the UI pass, any
    /// `show_viewport_immediate` calls render child windows via the registered
    /// renderer. Must be called inside `event_loop_context::with_event_loop_context`.
    fn render_root(&mut self, event_loop: &ActiveEventLoop) {
        let glstate = match &self.glstate {
            Some(g) => Rc::clone(g),
            None => return,
        };
        let painter = match &self.painter {
            Some(p) => Rc::clone(p),
            None => return,
        };
        let egui_ctx = self.egui_ctx.clone();
        let beginning = self.beginning;
        let app = match self.app.as_mut() {
            Some(a) => a,
            None => return,
        };

        paint_viewport(&glstate, &painter, &egui_ctx, ViewportId::ROOT, true, |ui| app.ui(ui), beginning);

        // Schedule next frame.
        let delay = self.repaint_delay;
        event_loop.set_control_flow(if delay.is_zero() {
            self.request_root_redraw();
            ControlFlow::Poll
        } else if let Some(at) = Instant::now().checked_add(delay) {
            ControlFlow::WaitUntil(at)
        } else {
            ControlFlow::Wait
        });
    }
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() || self.setup_error.is_some() {
            return;
        }

        let (glstate, gl, painter) = match bring_up_root(&self.title, event_loop) {
            Ok(t) => t,
            Err(e) => {
                self.setup_error = Some(e);
                event_loop.exit();
                return;
            }
        };
        let egui_ctx = glstate.egui_ctx.clone();
        let glstate = Rc::new(RefCell::new(glstate));
        let painter = Rc::new(RefCell::new(painter));

        // Wake the loop on any repaint request (including from the PipeWire thread).
        {
            let proxy = egui::mutex::Mutex::new(self.proxy.clone());
            egui_ctx.set_request_repaint_callback(move |info| {
                let _ = proxy.lock().send_event(UserEvent::Redraw(info.delay));
            });
        }

        // Register the immediate-viewport renderer (creates/renders child windows).
        {
            let glstate = Rc::clone(&glstate);
            let painter = Rc::clone(&painter);
            let beginning = self.beginning;
            egui::Context::set_immediate_viewport_renderer(move |ctx, immediate| {
                render_immediate_viewport(ctx, &glstate, &painter, beginning, immediate);
            });
        }

        // Show the root window and kick off the first frame.
        if let Some(vp) = glstate.borrow().viewports.get(&ViewportId::ROOT) {
            if let Some(w) = &vp.window {
                w.set_visible(true);
                w.request_redraw();
            }
        }

        // Build the user's app.
        let cc = CreationContext {
            gl: std::sync::Arc::clone(&gl),
            egui_ctx: egui_ctx.clone(),
            glstate: Rc::clone(&glstate),
        };
        match self.factory.take().map(|f| f(&cc)) {
            Some(Ok(app)) => self.app = Some(app),
            Some(Err(e)) => {
                self.setup_error = Some(e);
                event_loop.exit();
            }
            None => {}
        }

        self.glstate = Some(glstate);
        self.painter = Some(painter);
        self.gl = Some(gl);
        self.egui_ctx = egui_ctx;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        let vid = self
            .glstate
            .as_ref()
            .and_then(|g| g.borrow().viewport_from_window.get(&window_id).copied());

        match &event {
            WindowEvent::CloseRequested => {
                if vid == Some(ViewportId::ROOT) {
                    event_loop.exit();
                    return;
                }
                if let (Some(vid), Some(gs)) = (vid, self.glstate.as_ref()) {
                    let parent = {
                        let mut g = gs.borrow_mut();
                        match g.viewports.get_mut(&vid) {
                            Some(vp) => {
                                vp.info.events.push(egui::ViewportEvent::Close);
                                vp.parent
                            }
                            None => return,
                        }
                    };
                    self.egui_ctx.request_repaint_of(vid);
                    if let Some(parent) = parent {
                        self.egui_ctx.request_repaint_of(parent);
                    }
                    self.request_root_redraw();
                }
                return;
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    if let (Some(vid), Some(gs)) = (vid, self.glstate.as_ref()) {
                        gs.borrow_mut().resize(vid, *size);
                    }
                    self.request_root_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                event_loop_context::with_event_loop_context(event_loop, || {
                    self.render_root(event_loop);
                });
                return;
            }
            _ => {}
        }

        // Route the event to the target viewport's input state.
        let repaint = if let (Some(vid), Some(gs)) = (vid, self.glstate.as_ref()) {
            let mut g = gs.borrow_mut();
            let GlState { viewports, .. } = &mut *g;
            if let Some(vp) = viewports.get_mut(&vid) {
                if let (Some(window), Some(state)) = (vp.window.as_ref(), vp.egui_winit.as_mut()) {
                    state.on_window_event(window, &event).repaint
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if repaint {
            self.request_root_redraw();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw(delay) => {
                self.repaint_delay = delay;
                self.request_root_redraw();
            }
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = &cause {
            self.request_root_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let (Some(app), Some(gl)) = (self.app.as_mut(), self.gl.as_ref()) {
            app.on_exit(gl);
        }
        if let Some(painter) = self.painter.as_ref() {
            // Make the root surface current so the painter can free GL objects.
            if let Some(gs) = self.glstate.as_ref() {
                let g = gs.borrow();
                if let Some(vp) = g.viewports.get(&ViewportId::ROOT) {
                    if let Some(surface) = &vp.surface {
                        let _ = g.context.make_current(surface);
                    }
                }
            }
            painter.borrow_mut().destroy();
        }
    }
}

/// Called by egui (via the registered renderer) for each immediate viewport.
/// Creates its window if needed, runs its UI, paints it to its own window, and
/// handles its output. Re-entrant: the UI may show further immediate viewports.
fn render_immediate_viewport(
    egui_ctx: &egui::Context,
    glstate: &Rc<RefCell<GlState>>,
    painter: &Rc<RefCell<egui_glow::Painter>>,
    beginning: Instant,
    immediate: ImmediateViewport<'_>,
) {
    let ImmediateViewport {
        ids,
        builder,
        viewport_ui_cb,
    } = immediate;
    let vid = ids.this;

    // Ensure the window exists. (Hold no borrow across the user UI below.)
    {
        let mut gs = glstate.borrow_mut();
        gs.upsert_viewport(vid, Some(ids.parent), builder);
        let result =
            event_loop_context::with_current_event_loop(|event_loop| gs.ensure_window(vid, event_loop));
        match result {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                eprintln!("egui-pw-dmabuf: failed to open viewport window: {e}");
                return;
            }
            None => return, // no event loop available right now
        }
    }

    paint_viewport(&glstate, &painter, egui_ctx, vid, false, viewport_ui_cb, beginning);
}

/// Create the root window with a forced-EGL GL context, the shared egui context
/// (with viewport embedding disabled so immediate viewports become real
/// windows), and the painter.
fn bring_up_root(
    title: &str,
    event_loop: &ActiveEventLoop,
) -> Result<(GlState, std::sync::Arc<glow::Context>, egui_glow::Painter), BoxError> {
    let egui_ctx = egui::Context::default();
    egui_ctx.set_embed_viewports(false);

    let root_builder = ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([1024.0, 640.0]);

    let window_attributes =
        egui_winit::create_winit_window_attributes(&egui_ctx, root_builder.clone()).with_visible(false);

    let (mut window_opt, gl_config) = DisplayBuilder::new()
        .with_preference(ApiPreference::PreferEgl)
        .with_window_attributes(Some(window_attributes.clone()))
        .build(event_loop, ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false),
            |mut cfgs| cfgs.next().expect("no matching GL configuration"))
        .map_err(|e| e.to_string())?;

    let gl_display = gl_config.display();

    let raw_window_handle = match window_opt.as_ref() {
        Some(w) => Some(
            w.window_handle()
                .map_err(|e| -> BoxError { format!("window handle: {e}").into() })?
                .as_raw(),
        ),
        None => None,
    };
    let context_attributes = ContextAttributesBuilder::new().build(raw_window_handle);
    let fallback_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::Gles(None))
        .build(raw_window_handle);
    let not_current = unsafe {
        ctx!(
            gl_display
                .create_context(&gl_config, &context_attributes)
                .or_else(|_| gl_display.create_context(&gl_config, &fallback_attributes)),
            "create GL context"
        )?
    };

    let window = match window_opt.take() {
        Some(w) => w,
        None => ctx!(
            glutin_winit::finalize_window(event_loop, window_attributes, &gl_config),
            "finalize window"
        )?,
    };
    egui_winit::apply_viewport_builder_to_window(&egui_ctx, &window, &root_builder);

    let surface = ctx!(create_surface(&gl_display, &gl_config, &window), "create surface")?;
    let context = ctx!(not_current.make_current(&surface), "make current")?;
    let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::MIN));

    let gl = unsafe {
        glow::Context::from_loader_function(|s| {
            let cs = CString::new(s).expect("invalid proc name");
            gl_display.get_proc_address(&cs)
        })
    };
    let gl = std::sync::Arc::new(gl);

    let painter = ctx!(
        egui_glow::Painter::new(std::sync::Arc::clone(&gl), "", None, true),
        "create painter"
    )?;
    let max_texture_side = Some(painter.max_texture_side());

    let mut root_info = ViewportInfo::default();
    egui_winit::update_viewport_info(&mut root_info, &egui_ctx, &window, true);
    let root_state = egui_winit::State::new(
        egui_ctx.clone(), ViewportId::ROOT, event_loop,
        Some(window.scale_factor() as f32), event_loop.system_theme(),
        max_texture_side,
    );

    let window_id = window.id();
    let mut viewports = HashMap::new();
    viewports.insert(
        ViewportId::ROOT, Viewport {
            parent: None, builder: root_builder, info: root_info,
            window: Some(window), surface: Some(surface),
            egui_winit: Some(root_state),
        },
    );
    let mut viewport_from_window = HashMap::new();
    viewport_from_window.insert(window_id, ViewportId::ROOT);

    let glstate = GlState {
        display: gl_display, gl_config, context,
        max_texture_side, egui_ctx: egui_ctx.clone(),
        viewports, viewport_from_window,
    };
    Ok((glstate, gl, painter))
}
