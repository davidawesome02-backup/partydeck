
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::thread;

#[repr(C)]
#[derive(Debug)]
#[allow(non_snake_case)]
pub struct GamescopeWebrtcCtx {
    pub crl_path: *const c_char,
    pub kbm_path: *const c_char,

    pub ICE_offer: *const c_char,
    pub join_code: *const c_char,
    pub WEBRTC_connection_failed: bool,

    pub result_err: c_int,

    pub opaque_internal_ctx: *mut c_void,
}

extern "C" {
    fn gamescopeWebrtc_INIT( // ALL NAMES ARE WRONG HERE FIX DAVID GPT GENERATED THEM LOL
        create_kbm: bool,
        create_ctrl: bool,
    ) -> *mut GamescopeWebrtcCtx;

    fn gamescopeWebrtc_create_webrtc(
        ctx: *mut GamescopeWebrtcCtx,
        fps: c_int,
        create_code: bool,
        code_creation_url: *mut c_char,
    );

    fn gamescopeWebrtc_check_webrtc(
        ctx: *mut GamescopeWebrtcCtx,
    );

    fn gamescopeWebrtc_start_recording(
        ctx: *mut GamescopeWebrtcCtx,
        gamescope_pid: c_int,
    );
}



pub fn start_webrtc_stream() -> *mut GamescopeWebrtcCtx {
    println!("hi me");
    unsafe {
        let context: *mut GamescopeWebrtcCtx = gamescopeWebrtc_INIT(true, false);
        gamescopeWebrtc_create_webrtc(context, 60, true, CString::new("wss://webrtc-streaming-pages.pages.dev/websocket").expect("asd").as_ptr().cast_mut());

        context
    }
}

pub fn check_webrtc_stream_codes(context: *mut GamescopeWebrtcCtx) -> Option<String> {
    unsafe {
        gamescopeWebrtc_check_webrtc(context);
        if !(*context).join_code.is_null() {
            if let Ok(a) = CStr::from_ptr((*context).join_code).to_str() {
                return Some(a.to_string());
            }
        }
        None
    }
}



pub fn start_webrtc_streaming_thread(context: *mut GamescopeWebrtcCtx, pid: i32) -> thread::JoinHandle<()> {
    let ctx_adr = context as usize;
    thread::spawn(move || {
        unsafe {
            let context = ctx_adr as *mut GamescopeWebrtcCtx;
            gamescopeWebrtc_start_recording(context, pid);
        }
    })
}