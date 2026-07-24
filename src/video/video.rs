//! A zero-copy [`egui`] widget that displays a video stream received from
//! gamescope (or any compatible producer) over PipeWire as a **DMA-BUF**,
//! imported straight into an OpenGL texture via **EGL** - the pixels never
//! touch the CPU.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use eframe::egui;
use eframe::glow;
use eframe::glow::HasContext as _;
use pipewire as pw;

use super::egl::{EglApi, EglError, EglImage};

use crate::video::pipewire::PipewireCommand::ConnectVid;
use crate::video::pipewire::{PipewireCommand, PipewireID, PipewireStream};

/// Errors that can occur while constructing a [`PipewireVideo`].
#[derive(Debug)]
pub enum Error {
    Egl(EglError),
    Gl(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Egl(e) => write!(f, "EGL error: {e}"),
            Error::Gl(e) => write!(f, "GL error: {e}"),
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

/// GPU-side state for one video widget.
struct Renderer {
    egl: Arc<EglApi>,
    texture: glow::Texture,
    fbo: glow::Framebuffer,
    image: Option<EglImage>,
    size: (u32, u32),
    fbo_complete: bool,
    logged_no_display: bool,
}

unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    /// Create a new renderer. The `EglApi` is shared via `Arc` from `PartyApp`.
    fn new(egl: &Arc<EglApi>) -> Result<Self, Error> {
        let egl = Arc::clone(egl);
        let gl = &egl.gl_ctx;

        // Create GL texture
        let texture = unsafe { gl.create_texture() }.map_err(Error::Gl)?;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
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

    /// Import a new DMA-BUF frame, replacing the current image.
    fn import(&mut self, gl: &glow::Context, desc: RwLockReadGuard<PipewireStream>) {
        let display = match self.egl.current_display() {
            Ok(d) => d,
            Err(_) => {
                if !self.logged_no_display {
                    self.logged_no_display = true;
                    eprintln!("egl: no current EGLDisplay, DMA-BUF import skipped");
                }
                return;
            }
        };

        let new_image = match self.egl.create_dmabuf_image(display, &desc) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("egl: DMA-BUF import failed: {e}");
                return;
            }
        };

        if let Some(old) = self.image.take() {
            self.egl.destroy_image(display, old);
        }

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
                eprintln!("egl: framebuffer incomplete (0x{:04x}), skipping frame", status);
            }
        }

        self.image = Some(new_image);
        self.size = (desc.width, desc.height);
    }

    /// Render into `rect`. Called from the glow paint callback.
    fn paint(&mut self, gl: &glow::Context, info: &egui::PaintCallbackInfo, rect: egui::Rect, desc: RwLockReadGuard<PipewireStream>) {
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

        let dx0 = (rect.min.x * ppp).round() as i32;
        let dx1 = (rect.max.x * ppp).round() as i32;
        let dy0 = (screen_h - rect.max.y * ppp).round() as i32;
        let dy1 = (screen_h - rect.min.y * ppp).round() as i32;
        let (sy0, sy1) = (h as i32, 0);

        unsafe {
            let prev = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbo));
            gl.blit_framebuffer(
                0, sy0, w as i32, sy1,
                dx0, dy0, dx1, dy1,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            let restore = NonZeroU32::new(prev as u32).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, restore);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        if let Some(img) = self.image.take() {
            if let Ok(display) = self.egl.current_display() {
                self.egl.destroy_image(display, img);
            }
        }
        self.fbo_complete = false;
        unsafe {
            self.egl.gl_ctx.delete_framebuffer(self.fbo);
            self.egl.gl_ctx.delete_texture(self.texture);
        }
    }
}

pub struct PipewireVideo {
    renderer: Arc<Mutex<Renderer>>,
    sender: pw::channel::Sender<PipewireCommand>,
    pub pw_id: PipewireID,
    streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
}

impl PipewireVideo {
    pub fn new(
        egl: &Arc<EglApi>,
        target: PipewireID,
        sender: pw::channel::Sender<PipewireCommand>,
        streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
    ) -> Result<Self, Error> {
        let renderer = Renderer::new(egl)?;
        let renderer = Arc::new(Mutex::new(renderer));

        let _ = sender.send(ConnectVid(target));

        Ok(PipewireVideo {
            renderer,
            sender,
            pw_id: target,
            streams,
        })
    }

    /// Draw the widget at `desired_size`.
    pub fn ui(&mut self, ui: &mut egui::Ui, desired_size: egui::Vec2) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

        if !ui.is_rect_visible(rect) {
            return response;
        }

        let Some(pipewire_stream) = self.streams.read().ok()
            .and_then(|streams_lock| streams_lock.get(&self.pw_id).cloned())
        else {
            return response;
        };

        {
            let Ok(pipewire_stream) = pipewire_stream.read() else {
                return response;
            };
            if !pipewire_stream.streaming {
                return response;
            }
        }

        if ui.input(|i| i.pointer.latest_pos().is_some_and(|x| rect.contains(x))) {
            ui.set_cursor_icon(egui::CursorIcon::None);
        }

        let renderer = self.renderer.clone();
        let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| {
            let Ok(mut r) = renderer.lock() else { return; };
            let Ok(pipewire_stream) = pipewire_stream.read() else { return; };
            if !pipewire_stream.streaming { return; }
            let gl = painter.gl();
            r.paint(gl, &info, rect, pipewire_stream);
        });

        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });

        response
    }
}

impl Drop for PipewireVideo {
    fn drop(&mut self) {
        let _ = self.sender.send(PipewireCommand::Disconnect(self.pw_id));
    }
}
