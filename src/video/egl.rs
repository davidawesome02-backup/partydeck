//! Minimal EGL layer for importing a single-plane DMA-BUF into a GL texture.
//!
//! We deliberately load EGL *dynamically* (`libEGL.so.1`) via `khronos-egl`'s
//! `DynamicInstance`, as the project notes require. The DMA-BUF import itself
//! goes through the `EGL_EXT_image_dma_buf_import` extension, whose entry points
//! (`eglCreateImageKHR` / `eglDestroyImageKHR`) and the GL-side
//! `glEGLImageTargetTexture2DOES` are resolved at runtime with
//! `eglGetProcAddress` and called through hand-declared FFI signatures. Those
//! KHR/OES variants take an `EGLint` (i32) attribute list, which is exactly the
//! shape of the constants the build script derives from the system headers, so
//! we avoid the EGL 1.5 core `eglCreateImage` (which needs `EGLAttrib` and an
//! EGL 1.5 context) and stay compatible with EGL 1.4 drivers.

use std::ffi::c_void;
use std::fmt;
use std::sync::RwLockReadGuard;

use khronos_egl as egl;

use crate::video::PipewireStream;

/// EGL enum values and the DRM protocol constants. These live in the committed
/// file `src/egl_constants.rs`, which is generated once (out of band) by
/// `tools/gen_egl_constants.c` compiling against the real system headers and
/// printing the values. Nothing is generated, parsed, or guessed at build time;
/// the Rust build just `include!`s the checked-in file. `DRM_FORMAT_XRGB8888`
/// is the FourCC for gamescope's BGRx export (its in-memory byte order is
/// B, G, R, x, matching PipeWire's `BGRx`), and `DRM_FORMAT_MOD_LINEAR` is the
/// un-tiled layout gamescope mandates.
///
/// The modifier hi/lo attribute enums are only used by the optional
/// explicit-modifier import path, so `dead_code` is allowed for the module.
#[allow(dead_code)]
mod sys {
    include!("egl_constants.rs");
}

pub use sys::DRM_FORMAT_XRGB8888;

// ---- runtime-resolved extension entry points -------------------------------
//
// All pointer parameters are spelled as `*mut c_void` and enums/ints as
// `u32`/`i32`, which are exactly the underlying types of `khronos_egl`'s
// `EGLDisplay`/`EGLContext`/`EGLImage`/`Enum`/`Int` aliases. Using the raw
// types directly keeps these declarations independent of which aliases the
// crate happens to re-export.

type PfnEglCreateImageKhr = unsafe extern "C" fn(
    dpy: *mut c_void,
    ctx: *mut c_void,
    target: u32,
    buffer: *mut c_void,
    attrib_list: *const i32,
) -> *mut c_void;

type PfnEglDestroyImageKhr =
    unsafe extern "C" fn(dpy: *mut c_void, image: *mut c_void) -> u32;

type PfnGlEglImageTargetTexture2dOes =
    unsafe extern "C" fn(target: u32, image: *mut c_void);

/// Errors that can occur while bringing up EGL or importing a buffer.
#[derive(Debug)]
pub enum EglError {
    /// `libEGL.so.1` could not be loaded, or a required core symbol was absent.
    Load(String),
    /// A required extension entry point was not advertised by the driver.
    MissingProc(&'static str),
    /// No current `EGLDisplay` - `eglGetCurrentDisplay` returned none. This is
    /// expected only if we are called without the GL context being current.
    NoCurrentDisplay,
    /// `eglCreateImageKHR` failed. Carries the raw `eglGetError` code if known.
    CreateImage(Option<egl::Int>),
}

impl fmt::Display for EglError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EglError::Load(e) => write!(f, "failed to load libEGL: {e}"),
            EglError::MissingProc(p) => {
                write!(f, "EGL/GL driver does not provide required entry point `{p}`")
            }
            EglError::NoCurrentDisplay => {
                write!(f, "no current EGLDisplay (is the GL context current?)")
            }
            EglError::CreateImage(Some(code)) => {
                write!(f, "eglCreateImageKHR failed (eglGetError = 0x{code:04x})")
            }
            EglError::CreateImage(None) => write!(f, "eglCreateImageKHR failed"),
        }
    }
}

impl std::error::Error for EglError {}

/// A live `EGLImageKHR`. Tied logically to the display it was created on; the
/// caller is responsible for destroying it (via [`EglApi::destroy_image`])
/// before dropping, and for not using it after the owning fd is gone.
pub struct EglImage {
    raw: *mut c_void,
}

impl EglImage {
    #[inline]
    pub fn as_ptr(&self) -> *mut c_void {
        self.raw
    }
}

/// The loaded EGL instance plus the extension entry points we need.
pub struct EglApi {
    instance: egl::DynamicInstance<egl::EGL1_4>,
    create_image: PfnEglCreateImageKhr,
    destroy_image: PfnEglDestroyImageKhr,
    image_target_texture_2d: PfnGlEglImageTargetTexture2dOes,
}

impl EglApi {
    /// Load `libEGL.so.1` and resolve the dma-buf import entry points.
    ///
    /// Returns an error (rather than panicking) if EGL is unavailable or the
    /// driver lacks `EGL_EXT_image_dma_buf_import` / the OES texture target -
    /// this is what lets the widget constructor fail cleanly per the brief.
    pub fn load() -> Result<Self, EglError> {
        // SAFETY: `load_required` dlopen's the system EGL and reads its symbol
        // table; it is unsafe only in the usual FFI sense. We immediately
        // capture any failure as an error value.
        let instance = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }
            .map_err(|e| EglError::Load(e.to_string()))?;

        let create_image = unsafe {
            load_proc::<PfnEglCreateImageKhr>(&instance, "eglCreateImageKHR")
        }?;
        let destroy_image = unsafe {
            load_proc::<PfnEglDestroyImageKhr>(&instance, "eglDestroyImageKHR")
        }?;
        let image_target_texture_2d = unsafe {
            load_proc::<PfnGlEglImageTargetTexture2dOes>(
                &instance,
                "glEGLImageTargetTexture2DOES",
            )
        }?;

        Ok(EglApi {
            instance,
            create_image,
            destroy_image,
            image_target_texture_2d,
        })
    }

    /// The `EGLDisplay` current on this thread. Must be called with the GL
    /// context current (i.e. from inside the egui paint callback).
    pub fn current_display(&self) -> Result<egl::Display, EglError> {
        self.instance
            .get_current_display()
            .ok_or(EglError::NoCurrentDisplay)
    }

    /// Import a single-plane DMA-BUF as an `EGLImageKHR`.
    ///
    /// The descriptor's fd is borrowed only for the duration of this call;
    /// EGL dup's the underlying dma_buf reference internally, so the resulting
    /// image stays valid even after the fd is later closed.
    pub fn create_dmabuf_image(
        &self,
        display: egl::Display,
        desc: &RwLockReadGuard<PipewireStream>,
    ) -> Result<EglImage, EglError> {
        use std::os::fd::AsRawFd as _;

        // i32 (EGLint) attribute list - the KHR import variant. For a LINEAR
        // buffer we intentionally omit the PLANE0_MODIFIER_{LO,HI} attributes:
        // the base EGL_EXT_image_dma_buf_import path then assumes an implicit
        // modifier, which is both correct for linear and more widely supported
        // than requiring EGL_EXT_image_dma_buf_import_modifiers.

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

        // SAFETY: `display` is a valid current display; ctx is EGL_NO_CONTEXT
        // (null) and buffer is null, as mandated for the LINUX_DMA_BUF target.
        // The attribute list is i32-typed and NONE-terminated.
        let raw = unsafe {
            (self.create_image)(
                display.as_ptr(),
                std::ptr::null_mut(), // EGL_NO_CONTEXT
                sys::LINUX_DMA_BUF_EXT as u32,
                std::ptr::null_mut(), // no client buffer
                attribs.as_ptr(),
            )
        };

        if raw.is_null() {
            // EGL_NO_IMAGE_KHR == 0.
            let code = self.instance.get_error().map(|e| e as egl::Int);
            return Err(EglError::CreateImage(code));
        }

        Ok(EglImage { raw })
    }

    /// Respecify the currently-bound texture (on `target`, e.g.
    /// `glow::TEXTURE_2D`) so its storage aliases `image`. The texture must
    /// already be bound on the active texture unit. The caller passes the GL
    /// target constant so this module needs no GL binding of its own.
    ///
    /// # Safety
    /// A GL context must be current and a texture bound to `target`.
    pub unsafe fn bind_image_to_texture(&self, target: u32, image: &EglImage) {
        (self.image_target_texture_2d)(target, image.as_ptr());
    }

    /// Destroy an image previously returned by [`Self::create_dmabuf_image`].
    pub fn destroy_image(&self, display: egl::Display, image: EglImage) {
        // SAFETY: `image.raw` came from eglCreateImageKHR on this display and
        // is destroyed exactly once (it is consumed by value here).
        unsafe {
            let _ = (self.destroy_image)(display.as_ptr(), image.as_ptr());
        }
    }
}

/// Resolve an entry point via `eglGetProcAddress` and transmute it to `F`.
///
/// # Safety
/// `F` must be a function-pointer type whose signature matches the real entry
/// point's ABI. On Linux `extern "system"` and `extern "C"` are identical, so
/// transmuting the returned pointer to an `extern "C"` fn type is sound.
unsafe fn load_proc<F: Copy>(
    instance: &egl::DynamicInstance<egl::EGL1_4>,
    name: &'static str,
) -> Result<F, EglError> {
    let raw: extern "system" fn() = instance
        .get_proc_address(name)
        .ok_or(EglError::MissingProc(name))?;
    debug_assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<extern "system" fn()>(),
        "fn-pointer size mismatch while loading {name}"
    );
    Ok(std::mem::transmute_copy::<extern "system" fn(), F>(&raw))
}
