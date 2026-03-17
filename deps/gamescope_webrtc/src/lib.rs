use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::OnceLock;
use std::thread;

use libloading::Library;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(non_snake_case)]
pub struct gamescope_webrtc_ctx {
    pub crl_path: *const c_char,
    pub kbm_path: *const c_char,

    pub ICE_offer: *const c_char,
    pub join_code: *const c_char,
    pub webrtc_connection_failed: bool,

    pub result_err: c_int,

    pub opaque_internal_ctx: *mut c_void,
}

macro_rules! webrtc_api {
    ($(
        fn $name:ident ( $($arg:ty),* ) -> $ret:ty;
    )*) => {
        #[allow(non_snake_case)]
        pub struct WebrtcLib {
            _lib: Library,
            $(pub $name: unsafe extern "C" fn($($arg),*) -> $ret,)*
        }

        impl WebrtcLib {
            unsafe fn load() -> Result<Self, String> {
                println!("[webrtc] Attempting to load libgamescope-webrtc-lib.so");

                let lib = match Library::new("libgamescope-webrtc-lib.so") {
                    Ok(l) => {
                        println!("[webrtc] Loaded from system loader path");
                        l
                    }
                    Err(e1) => {
                        println!("[webrtc] Failed system lookup: {}", e1);
                        match Library::new("./libgamescope-webrtc-lib.so") {
                            Ok(l) => {
                                println!("[webrtc] Loaded from current directory");
                                l
                            }
                            Err(e2) => {
                                println!("[webrtc] Failed CWD lookup: {}", e2);
                                return Err(format!(
                                    "Could not load libgamescope-webrtc-lib.so\nsystem loader: {}\nCWD: {}",
                                    e1, e2
                                ));
                            }
                        }
                    }
                };

                $(
                    let $name = match lib.get::<unsafe extern "C" fn($($arg),*) -> $ret>(
                        concat!(stringify!($name), "\0").as_bytes()
                    ) {
                        Ok(sym) => {
                            println!("[webrtc] Loaded symbol {}", stringify!($name));
                            *sym
                        }
                        Err(e) => {
                            return Err(format!(
                                "Missing symbol {}: {}",
                                stringify!($name),
                                e
                            ));
                        }
                    };
                )*

                println!("[webrtc] WebRTC library successfully loaded");

                Ok(Self {
                    _lib: lib,
                    $($name,)*
                })
            }
        }
    };
}


webrtc_api! {
    fn gamescope_webrtc_init(bool, bool) -> *mut gamescope_webrtc_ctx;
    fn gamescope_webrtc_create_webrtc(*mut gamescope_webrtc_ctx, c_int, bool, *mut c_char) -> ();
    fn gamescope_webrtc_check_webrtc(*mut gamescope_webrtc_ctx) -> ();
    fn gamescope_webrtc_start_recording(*mut gamescope_webrtc_ctx, c_int) -> ();
}

static WEBRTC_LIB: OnceLock<Option<WebrtcLib>> = OnceLock::new();


fn get_lib() -> Option<&'static WebrtcLib> {
    WEBRTC_LIB
        .get_or_init(|| {
            match unsafe { WebrtcLib::load() } {
                Ok(lib) => {
                    println!("[webrtc] WebRTC support ENABLED");
                    Some(lib)
                }
                Err(e) => {
                    println!("[webrtc] WebRTC support DISABLED: {}", e);
                    None
                }
            }
        })
        .as_ref()
}

pub fn webrtc_available() -> bool {
    get_lib().is_some()
}

pub fn start_webrtc_stream() -> Option<*mut gamescope_webrtc_ctx> {
    let lib = get_lib()?;

    unsafe {
        let context = (lib.gamescope_webrtc_init)(true, false);

        let url = CString::new("wss://webrtc-streaming-pages.pages.dev/websocket").unwrap();

        (lib.gamescope_webrtc_create_webrtc)(
            context,
            60,
            true,
            url.as_ptr().cast_mut(),
        );

        Some(context)
    }
}

pub fn check_webrtc_stream_codes(
    context: *mut gamescope_webrtc_ctx,
) -> Option<String> {
    let lib = get_lib()?;

    unsafe {
        (lib.gamescope_webrtc_check_webrtc)(context);

        if !(*context).join_code.is_null() {
            if let Ok(a) = CStr::from_ptr((*context).join_code).to_str() {
                return Some(a.to_string());
            }
        }

        None
    }
}

pub fn start_webrtc_streaming_thread(
    context: *mut gamescope_webrtc_ctx,
    pid: u32,
) -> Option<thread::JoinHandle<()>> {
    let lib = get_lib()?;

    let ctx_adr = context as usize;
    let start_recording = lib.gamescope_webrtc_start_recording;

    Some(thread::spawn(move || {
        unsafe {
            std::thread::sleep(std::time::Duration::from_secs(5));

            let context = ctx_adr as *mut gamescope_webrtc_ctx;
            (start_recording)(context, pid as i32);
        }
    }))
}