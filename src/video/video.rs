//! A zero-copy [`egui`] widget that displays a video stream received from
//! gamescope (or any compatible producer) over PipeWire as a **DMA-BUF**,
//! imported straight into an OpenGL texture via **EGL** - the pixels never
//! touch the CPU.
//!
//! # How it works
//!
//! A [`PipewireVideo`] owns a background thread running a PipeWire main loop
//! (see [`pipewire_thread`]). When you call [`PipewireVideo::play`] the thread
//! connects an input stream and negotiates a single-plane BGRx format carrying a
//! `LINEAR` DMA-BUF (the same handshake gamescope's own capture path performs).
//! For every captured frame the thread keeps only the newest buffer, dup's the
//! plane's file descriptor into a [`frame::FrameDescriptor`], and publishes it
//! into shared state, then asks egui to repaint.
//!
//! On the render side, inside an [`egui::PaintCallback`] (where the GL context
//! is current), the [`Renderer`] picks up the newest descriptor - if one has
//! arrived - imports it with `eglCreateImageKHR`, binds the resulting
//! `EGLImage` to a persistent GL texture with `glEGLImageTargetTexture2DOES`,
//! and blits that texture into the widget's rectangle with `glBlitFramebuffer`
//! (which scales to the egui-allocated size and needs no shaders or geometry).
//! Because the renderer only swaps frames when a new one is available, the egui
//! and PipeWire frame rates are fully decoupled: a slow, paused, or disconnected
//! producer simply leaves the last frame on screen.
//!
//! # Headless note
//!
//! This crate is designed against the real EGL/glow/PipeWire APIs and gamescope's
//! actual negotiation logic, but it can only *run* on a host that has a GPU, a
//! GL context (provided by this crate's built-in runner), and a PipeWire video
//! producer. See the README for run instructions and the handful of things to
//! verify on real hardware (notably the vertical flip and the modifier path).

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::thread::JoinHandle;

use eframe::*;

use glow::HasContext as _;
use pipewire as pw;

// mod app;
// mod egl;
// mod frame;
// mod pipewire_thread;

use super::egl::{EglApi, EglError, EglImage};


// pub use app::{run, App, BoxError, CreationContext};

use crate::video::pipewire::PipewireCommand::ConnectVid;
use crate::video::{
    pipewire::{PipewireCommand, PipewireID, PipewireStream},
    app_wrapper::CreationContext
};

// Re-exported so a consumer can build their UI and handle GL teardown using the
// exact versions this crate links, without adding egui/glow to their Cargo.toml.
use {egui, glow};

/// Errors that can occur while constructing a [`PipewireVideo`].
///
/// The constructor returns one of these - rather than panicking - for any
/// failure to bring up EGL, resolve the dma-buf import entry points, or create
/// the backing GL objects, exactly as the design requires.
#[derive(Debug)]
pub enum Error {
    /// Reserved: no glow (OpenGL) context was available. The crate's runner
    /// always provides one, so this is not normally produced.
    NoGlowContext,
    /// Bringing up EGL, loading `libEGL.so.1`, or resolving a required
    /// extension entry point failed.
    Egl(EglError),
    /// A GL object (texture or framebuffer) could not be created.
    Gl(String),
    /// The PipeWire capture thread could not be spawned.
    Thread(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoGlowContext => write!(
                f,
                "no glow/OpenGL context available"
            ),
            Error::Egl(e) => write!(f, "EGL initialization failed: {e}"),
            Error::Gl(e) => write!(f, "failed to create GL resources: {e}"),
            Error::Thread(e) => write!(f, "failed to start capture thread: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Egl(e) => Some(e),
            _ => None,
        }
    }
}

impl From<EglError> for Error {
    fn from(e: EglError) -> Self {
        Error::Egl(e)
    }
}

/// The GPU-side state for one video widget: the EGL import layer plus a
/// persistent texture and framebuffer that the imported DMA-BUF is aliased into
/// and blitted from.
///
/// All of its methods take the `glow` context explicitly; it never stores one.
/// At render time the context comes from the paint callback's [`egui_glow::Painter`];
/// at teardown it comes from the app's `on_exit`.
struct Renderer {
    egl: EglApi,
    /// Persistent texture; re-pointed at each new DMA-BUF via `EGLImage`.
    texture: glow::Texture,
    /// Persistent FBO with `texture` attached as color attachment 0; used as the
    /// blit source.
    fbo: glow::Framebuffer,
    /// The live image backing `texture`, if any. Destroyed when replaced.
    image: Option<EglImage>,
    /// Size of the current image in texels.
    size: (u32, u32),
    /// Whether `fbo` is framebuffer-complete with the current image (and thus
    /// safe to blit from).
    fbo_complete: bool,
    /// Set once we have logged the "no EGL display" condition, so we warn at
    /// most once instead of every frame.
    logged_no_display: bool,
}

// SAFETY: `Renderer` holds raw EGL handles (an `EGLImage` and, transitively, an
// `EGLDisplay`) that are not in themselves thread-safe. We wrap every `Renderer`
// in an `Arc<Mutex<..>>` and only ever touch it from the runner's main thread:
// once in the constructor (GL context current) and once per frame inside the
// glow paint callback, which egui_glow always invokes on that same main thread.
// The `Send + Sync` bound exists solely because `egui_glow::CallbackFn::new`
// requires its closure (which captures the `Arc<Mutex<Renderer>>`) to be
// `Send + Sync`; it does not reflect any actual cross-thread use of the GL or
// EGL handles. The `Mutex` further serializes access.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    fn new(gl: &glow::Context) -> Result<Self, Error> {
        // Load EGL and the dma-buf import entry points first: if the driver
        // can't do the import, fail before we allocate anything.
        let egl = EglApi::load()?;

        // SAFETY: a GL context is current here (the runner calls the app
        // factory with the context current). These are ordinary GL object
        // creation/parameter calls.
        let texture = unsafe { gl.create_texture() }.map_err(Error::Gl)?;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        let fbo = unsafe { gl.create_framebuffer() }.map_err(Error::Gl)?;

        Ok(Renderer {
            egl,
            texture,
            fbo,
            image: None,
            size: (0, 0),
            fbo_complete: false,
            logged_no_display: false,
        })
    }

    /// Import a freshly captured DMA-BUF, replacing the current image. On any
    /// failure the previous image is left in place so the widget keeps showing
    /// the last good frame.
    fn import(&mut self, gl: &glow::Context, desc: RwLockReadGuard<PipewireStream>) {
        // We need the EGLDisplay that the current GL context belongs to. This is
        // `eglGetCurrentDisplay()`; it only returns a display when the context is
        // an EGL context. This crate's runner (`crate::run`) forces an EGL
        // context on both X11 and Wayland, so this normally succeeds. We must
        // NOT substitute a separately created/initialized display here: that is a
        // different EGL connection than the context, so importing into it yields
        // a null image and then crashes when the texture is used. If there is no
        // current display (e.g. an embedding app gave us a GLX context, or EGL
        // fell back to GLX), we warn once and keep showing the last frame.
        let display = match self.egl.current_display() {
            Ok(d) => d,
            Err(_) => {
                if !self.logged_no_display {
                    self.logged_no_display = true;
                    eprintln!(
                        "egui-pw-dmabuf: eglGetCurrentDisplay() returned EGL_NO_DISPLAY, so the \
                         current GL context is not an EGL context. DMA-BUF import requires EGL. \
                         If you are using this crate's runner this should not happen; if you are \
                         embedding the widget in another framework, ensure it creates an EGL (not \
                         GLX) context. The video will stay blank until then."
                    );
                }
                return;
            }
        };

        // Build the replacement before tearing down the old one.
        let new_image = match self.egl.create_dmabuf_image(display, &desc) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("egui-pw-dmabuf: DMA-BUF import failed: {e}");
                return;
            }
        };

        if let Some(old) = self.image.take() {
            self.egl.destroy_image(display, old);
        }

        // SAFETY: GL context current; `texture`/`fbo` are valid objects created
        // in `new`. We restore both framebuffer bindings to 0 before returning,
        // which is the state egui_glow expects on callback entry/continuation.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            self.egl.bind_image_to_texture(glow::TEXTURE_2D, &new_image);
            gl.bind_texture(glow::TEXTURE_2D, None);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(self.texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            self.fbo_complete = status == glow::FRAMEBUFFER_COMPLETE;
            if !self.fbo_complete {
                eprintln!(
                    "egui-pw-dmabuf: framebuffer incomplete after import \
                     (status = 0x{status:04x}); frame will be skipped"
                );
            }
        }

        self.image = Some(new_image);
        self.size = (desc.width, desc.height);
        // `desc` (and its dup'd fd) is dropped here. EGL holds its own reference
        // to the underlying dma_buf, so the image stays valid.
    }

    /// Render the current frame into `rect`. Called from the glow paint
    /// callback with the GL context current and the draw framebuffer bound to
    /// the screen (framebuffer 0).
    fn paint(&mut self, gl: &glow::Context, info: &egui::PaintCallbackInfo, rect: egui::Rect, desc: RwLockReadGuard<PipewireStream>) {
        // Pick up a newer frame if one is waiting; otherwise keep the current
        // texture (this is what decouples the two frame rates).
        self.import(gl, desc);

        if self.image.is_none() || !self.fbo_complete {
            return;
        }
        let (w, h) = self.size;
        if w == 0 || h == 0 {
            return;
        }

        let ppp = info.pixels_per_point;
        let screen_h = info.screen_size_px[1] as f32;

        // Destination in framebuffer pixels. egui rects are in points with a
        // top-left origin; GL window space has a bottom-left origin, so we flip
        // Y by measuring from `screen_h`.
        let dx0 = (rect.min.x * ppp).round() as i32;
        let dx1 = (rect.max.x * ppp).round() as i32;
        let dy0 = (screen_h - rect.max.y * ppp).round() as i32;
        let dy1 = (screen_h - rect.min.y * ppp).round() as i32;

        // Source in texels. Swapping y0/y1 flips the image vertically.
        let (sy0, sy1) = (h as i32, 0);

        // SAFETY: GL context current. We bind our FBO only as the READ target
        // (leaving the screen as the DRAW target), blit, then restore the
        // previous read-framebuffer binding.
        unsafe {
            let prev = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
            gl.blit_framebuffer(
                0,
                sy0,
                w as i32,
                sy1,
                dx0,
                dy0,
                dx1,
                dy1,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            let restore = NonZeroU32::new(prev as u32).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, restore);
        }
    }

    /// Destroy all GPU resources. Best-effort: the `EGLImage` is only freed if a
    /// display is still current.
    fn destroy(&mut self, gl: &glow::Context) {
        if let Some(img) = self.image.take() {
            if let Ok(display) = self.egl.current_display() {
                self.egl.destroy_image(display, img);
            }
        }
        self.fbo_complete = false;
        // SAFETY: GL context current (called from `on_exit`); objects valid and
        // deleted exactly once.
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.texture);
        }
    }
}

/// A reusable egui widget that displays a gamescope DMA-BUF video stream over
/// PipeWire, entirely on the GPU.
///
/// Construct one per stream with [`PipewireVideo::new`], call
/// [`play`](Self::play) to start capturing, and draw it each frame with
/// [`ui`](Self::ui). Multiple instances can coexist; each is rendered once per
/// egui frame. Remember to call [`destroy_gl_resources`](Self::destroy_gl_resources)
/// from your app's [`on_exit`](App::on_exit) to release GPU objects while the
/// GL context is still alive.
pub struct PipewireVideo {
    renderer: Arc<Mutex<Renderer>>,
    sender: pw::channel::Sender<PipewireCommand>,
    pub pw_id: PipewireID,
    streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
}

impl PipewireVideo {
    /// Create a widget, defaulting to a vertical flip (correct for gamescope's
    /// top-left-origin frames).
    ///
    /// Call this from your app factory (the closure passed to [`crate::run`]) so
    /// that the GL context is current. Returns [`Error`] if EGL or the dma-buf
    /// import extensions are unavailable, or if GL objects or the capture thread
    /// cannot be created.
    pub fn new(cc: &CreationContext, target: PipewireID, sender: pw::channel::Sender<PipewireCommand>, streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>) -> Result<Self, Error> {
        // let Some(ref gl) = cc.gl else {return Err(Error::NoGlowContext);};
        let gl = cc.gl.as_ref();
        let renderer = Renderer::new(&gl)?;
        let renderer = Arc::new(Mutex::new(renderer));

        sender.send(ConnectVid(target)); //todo properly do this.

        Ok(
            PipewireVideo {
                renderer,
                sender,
                pw_id: target,
                streams,
            }
        )
    }

    /// Whether the underlying PipeWire stream is currently in the streaming
    /// state.
    // pub fn connected(&self) -> bool {
    //     self.shared.connected()
    // }

    /// The most recently negotiated frame size in pixels, or `None` before the
    /// first frame has been negotiated. Useful for preserving aspect ratio.
    // pub fn native_size(&self) -> Option<(u32, u32)> {
    //     let (w, h) = self.shared.size();
    //     (w != 0 && h != 0).then_some((w, h))
    // }

    /// Draw the widget at `desired_size`, returning the [`egui::Response`] for
    /// the allocated rectangle. The video is scaled (via the blit) to fill the
    /// allocated rect, honoring egui's layout.
    pub fn ui(&mut self, ui: &mut egui::Ui, desired_size: egui::Vec2) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

        if !ui.is_rect_visible(rect) { return response; }

        let Some(pipewire_stream) = self.streams.read().ok().and_then(|streams_lock| {streams_lock.get(&self.pw_id).cloned()}) else {
            // Disconnected UI - WE SHOULD NEVER GET HERE
            return response;
        };

        // TODO maybe add pipewire disconnected UI check in here becasue we have not inspected the inside of pipewire_stream yet so we dont have to lock it.
        { // I dont want to manualy drop, so just block scope this.
            // Hate locking here because its a bit of a waste, but we have to to show the UI not inside GL.
            let Ok(pipewire_stream) = pipewire_stream.read() else {
                // Disconnected UI, or crash because lock poisoned
                return response;
            };

            if !pipewire_stream.streaming {
                return response;
            }

        }

        let renderer = self.renderer.clone();
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let Ok(mut r) = renderer.lock() else {return;};
            
            let Ok(pipewire_stream) = pipewire_stream.read() else {return;};

            let gl: &glow::Context = painter.gl();
            r.paint(gl, &info, rect, pipewire_stream);
        });

        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });
        
        response
    }

    // TODO, most likely remove...? or move to drop?

    /// Release all GPU resources. Call this from your [`App::on_exit`] while the
    /// GL context is still current. After this the widget must not be drawn again.
    pub fn destroy_gl_resources(&self, gl: &glow::Context) {
        if let Ok(mut r) = self.renderer.lock() {
            r.destroy(gl);
        }
    }
}

impl Drop for PipewireVideo {
    fn drop(&mut self) {
        // Ask the capture thread's loop to quit, then join it so the thread and
        // its PipeWire resources are gone before we return.
        let _ = self.sender.send(PipewireCommand::Disconnect(self.pw_id));
    }
}
