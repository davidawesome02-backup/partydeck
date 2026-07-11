//! Minimal EGL layer for importing a single-plane DMA-BUF into a GL texture.
//!
//! One `EglApi` is constructed per GL context and shared (via `Arc`) among all
//! video players. All function pointers are resolved through `get_proc_address`
//! from eframe's `CreationContext` — no `dlopen` or EGL wrapper crate needed.

use std::ffi::{c_void, CStr};
use std::fmt;
use std::sync::{Arc, RwLockReadGuard};

use crate::video::pipewire::PipewireStream;

/// The proc-address resolver from eframe's `CreationContext`.
pub type GetProcAddr = Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>;

/// EGL enum values and the DRM protocol constants. Generated once by
/// `tools/gen_egl_constants.c` from real system headers.
#[allow(dead_code)]
mod sys {
    include!("egl_constants.rs");
}

use eframe::glow;
pub use sys::DRM_FORMAT_XRGB8888;

// ---- FFI function-pointer types -------------------------------------------

type PfnEglCreateImageKhr = unsafe extern "C" fn(
    dpy: *mut c_void,
    ctx: *mut c_void,
    target: u32,
    buffer: *mut c_void,
    attrib_list: *const i32,
) -> *mut c_void;

type PfnEglDestroyImageKhr = unsafe extern "C" fn(dpy: *mut c_void, image: *mut c_void) -> u32;

type PfnGlEglImageTargetTexture2dOes = unsafe extern "C" fn(target: u32, image: *mut c_void);

/// Errors from EGL construction or DMA-BUF import.
#[derive(Debug)]
pub enum EglError {
    MissingProc(&'static str),
    NoCurrentDisplay,
    CreateImage(Option<u32>),
}

impl fmt::Display for EglError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EglError::MissingProc(p) => write!(f, "missing proc: {p}"),
            EglError::NoCurrentDisplay => {
                write!(f, "no current EGLDisplay (GL context not current?)")
            }
            EglError::CreateImage(Some(code)) => {
                write!(f, "eglCreateImageKHR failed (error 0x{code:04x})")
            }
            EglError::CreateImage(None) => write!(f, "eglCreateImageKHR failed"),
        }
    }
}

impl std::error::Error for EglError {}

/// A live `EGLImageKHR`. Tied logically to the display it was created on.
pub struct EglImage {
    raw: *mut c_void,
}

impl EglImage {
    #[inline]
    pub fn as_ptr(&self) -> *mut c_void {
        self.raw
    }
}

/// Shared EGL API for DMA-BUF import. Constructed once and shared via
/// `Arc` among all video players.
///
/// All entry points are resolved through the `get_proc_address` callback from
/// eframe's `CreationContext`, which covers both GL and EGL symbols.
pub struct EglApi {
    pub gl_ctx: std::sync::Arc<glow::Context>,
    /// `eglGetCurrentDisplay`
    egl_get_current_display: unsafe extern "C" fn() -> *mut c_void,
    /// `eglGetError`
    egl_get_error: unsafe extern "C" fn() -> u32,
    /// `eglCreateImageKHR`
    create_image: PfnEglCreateImageKhr,
    /// `eglDestroyImageKHR`
    destroy_image: PfnEglDestroyImageKhr,
    /// `glEGLImageTargetTexture2DOES`
    image_target_texture_2d: PfnGlEglImageTargetTexture2dOes,
}

/// Resolve a symbol via the GL display's `get_proc_address`.
/// # Safety
/// The returned pointer must be cast to the target function type.
/// # Safety
/// The returned pointer must be cast to the target function type.
/// We use `transmute_copy` so the compiler doesn't try to verify size equality
/// between `*const c_void` and the fn type (they're always the same size).
unsafe fn resolve_fn<F>(get_proc_address: &GetProcAddr, name: &str) -> Option<F> {
    let cs = std::ffi::CString::new(name).ok()?;
    let ptr = get_proc_address(&cs);
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&ptr))
    }
}

impl EglApi {
    /// Construct the EGL API from a `get_proc_address` callback.
    ///
    /// All functions are resolved through the callback — no `dlopen` or EGL
    /// wrapper crate is used.
    ///
    /// # Errors
    /// Returns `EglError::MissingProc` if a required function symbol is absent.
    pub fn new(gl_ctx: std::sync::Arc<glow::Context>, get_proc_address: &GetProcAddr) -> Result<Self, EglError> {
        let egl_get_current_display = unsafe {
            resolve_fn(get_proc_address, "eglGetCurrentDisplay")
                .ok_or_else(|| EglError::MissingProc("eglGetCurrentDisplay"))?
        };
        let egl_get_error = unsafe {
            resolve_fn(get_proc_address, "eglGetError")
                .ok_or_else(|| EglError::MissingProc("eglGetError"))?
        };
        let create_image = unsafe {
            resolve_fn(get_proc_address, "eglCreateImageKHR")
                .ok_or_else(|| EglError::MissingProc("eglCreateImageKHR"))?
        };
        let destroy_image = unsafe {
            resolve_fn(get_proc_address, "eglDestroyImageKHR")
                .ok_or_else(|| EglError::MissingProc("eglDestroyImageKHR"))?
        };
        let image_target_texture_2d = unsafe {
            resolve_fn(get_proc_address, "glEGLImageTargetTexture2DOES")
                .ok_or_else(|| EglError::MissingProc("glEGLImageTargetTexture2DOES"))?
        };

        Ok(EglApi {
            gl_ctx: gl_ctx.clone(),
            egl_get_current_display,
            egl_get_error,
            create_image,
            destroy_image,
            image_target_texture_2d,
        })
    }

    /// The `EGLDisplay` current on this thread.
    /// Must be called with the GL context current (from inside an egui paint callback).
    pub fn current_display(&self) -> Result<*mut c_void, EglError> {
        let dpy = unsafe { (self.egl_get_current_display)() };
        if dpy.is_null() {
            Err(EglError::NoCurrentDisplay)
        } else {
            Ok(dpy)
        }
    }

    /// Import a single-plane DMA-BUF as an `EGLImageKHR`.
    pub fn create_dmabuf_image(
        &self,
        display: *mut c_void,
        desc: &RwLockReadGuard<PipewireStream>,
    ) -> Result<EglImage, EglError> {
        use std::os::fd::AsRawFd as _;

        let attribs: [i32; 13] = [
            sys::WIDTH,
            desc.width as i32,
            sys::HEIGHT,
            desc.height as i32,
            sys::LINUX_DRM_FOURCC_EXT,
            DRM_FORMAT_XRGB8888 as i32,
            sys::DMA_BUF_PLANE0_FD_EXT,
            desc.dmabuf_latest as i32,
            sys::DMA_BUF_PLANE0_OFFSET_EXT,
            desc.offset as i32,
            sys::DMA_BUF_PLANE0_PITCH_EXT,
            desc.stride,
            sys::NONE,
        ];

        // SAFETY: `display` is the current EGL display; ctx and buffer are null
        // as mandated for the LINUX_DMA_BUF target.
        let raw = unsafe {
            (self.create_image)(
                display,
                std::ptr::null_mut(),
                sys::LINUX_DMA_BUF_EXT as u32,
                std::ptr::null_mut(),
                attribs.as_ptr(),
            )
        };

        if raw.is_null() {
            let code = unsafe { (self.egl_get_error)() };
            return Err(EglError::CreateImage(Some(code)));
        }

        Ok(EglImage { raw })
    }

    /// Bind `image` to the currently-bound texture on `target` (e.g. `glow::TEXTURE_2D`).
    ///
    /// # Safety
    /// A GL context must be current and a texture bound to `target`.
    pub unsafe fn bind_image_to_texture(&self, target: u32, image: &EglImage) {
        (self.image_target_texture_2d)(target, image.as_ptr());
    }

    /// Destroy an image previously returned by [`Self::create_dmabuf_image`].
    pub fn destroy_image(&self, display: *mut c_void, image: EglImage) {
        unsafe { let _ = (self.destroy_image)(display, image.as_ptr()); }
    }
}
