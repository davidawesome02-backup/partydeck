use std::{
    collections::HashMap,
    ffi::{CString, c_void},
    os::fd::{BorrowedFd, IntoRawFd, RawFd},
    path::PathBuf,
    ptr,
    sync::{
        Arc, Mutex, RwLock as StdRwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::video::pipewire::{
    DmaBufFrame, PipewireID, PipewireInstance, PipewireListener, PipewireStream,
};
use anyhow::{Context as _, Result};
use ffmpeg_next::{codec, dictionary, ffi, format::Pixel, frame, rational::Rational};
use nix::unistd;
use pipewire::channel::Sender;

static ENCODER_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

const TARGET_FPS: u64 = 60;
const FRAME_DURATION: Duration =
    Duration::from_nanos((1_000_000_000 + TARGET_FPS - 1) / TARGET_FPS);
const RTP_CLOCK_RATE: i32 = 90_000;
const PTS_STEP: i64 = RTP_CLOCK_RATE as i64 / TARGET_FPS as i64;

// const DRM_FORMAT_XRGB8888: u32 = u32::from_be_bytes(*b"XR24");
const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

fn chk(ret: i32, what: &'static str) -> Result<()> {
    if ret < 0 {
        Err(ffmpeg_next::Error::from(ret)).context(what)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HwBackend {
    #[default]
    Vaapi,
    Vulkan,
}
impl HwBackend {
    fn hw_pix_fmt(self) -> Pixel {
        match self {
            Self::Vaapi => Pixel::VAAPI,
            Self::Vulkan => Pixel::VULKAN,
        }
    }
    fn pix_fmt_name(self) -> &'static str {
        match self {
            Self::Vaapi => "vaapi",
            Self::Vulkan => "vulkan",
        }
    }
    fn convert_filter(self) -> (&'static str, &'static str) {
        match self {
            Self::Vaapi => ("scale_vaapi", "format=nv12"),
            Self::Vulkan => ("scale_vulkan", "format=nv12"),
        }
    }
    fn device_type(self) -> ffi::AVHWDeviceType {
        match self {
            Self::Vaapi => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
            Self::Vulkan => ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN,
        }
    }
    fn encoder_name(self) -> &'static str {
        match self {
            Self::Vaapi => "h264_vaapi",
            Self::Vulkan => "h264_vulkan",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct EncoderOptions {
    pub backend: HwBackend,
    pub device: Option<PathBuf>,
    pub bitrate_bps: u32,
}

/// Callback receives encoded H.264 packet data as &[u8].
/// The data is valid only during the callback; copy if needed.
/// Arguments: (packet_data, pts_90khz, is_keyframe)
pub type EncoderCallback = Box<dyn FnMut(&[u8], i64, bool) + Send + Sync + 'static>;

/// Frame data passed from PipeWire callback to encoder thread.
/// Just a notification - encoder thread reads latest frame from stream state.
type FrameSignal = ();

struct EncoderInner {
    callbacks: Arc<Mutex<HashMap<u64, EncoderCallback>>>,
    /// Single-slot channel: only a notification that a new frame is available.
    /// Encoder thread reads latest frame directly from PipeWire stream state.
    latest_frame_tx: SyncSender<FrameSignal>,
    keyframe_requested: Arc<AtomicBool>,
    _pw_listener: PipewireListener,
    _encoder_thread: JoinHandle<()>,
}


use std::sync::RwLock;
use crate::video::pipewire::PipewireCommand;

pub struct EncoderRegistry {
    pw_channel: Sender<PipewireCommand>,
    pw_streams: Arc<RwLock<HashMap<u32, Arc<RwLock<PipewireStream>>>>>,
    encoders: Arc<Mutex<HashMap<PipewireID, EncoderInner>>>,
}

impl EncoderRegistry {
    pub fn new(pipewire: &PipewireInstance) -> Self {
        Self {
            pw_channel: pipewire.channel.clone(),
            pw_streams: pipewire.streams.clone(),
            encoders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a WebRTC consumer for a PipeWire stream.
    /// Callback receives (packet_data, pts_90khz, is_keyframe).
    pub fn listen(
        &self,
        pw_id: PipewireID,
        callback: EncoderCallback,
    ) -> Result<EncoderReference> {
        let encoder_id = ENCODER_ID_COUNTER.fetch_add(1, Ordering::SeqCst);

        let mut map = self.encoders.lock().unwrap();
        let inner = map.entry(pw_id).or_insert_with(|| {
            let (latest_frame_tx, latest_frame_rx) = mpsc::sync_channel::<FrameSignal>(1);
            let callbacks: Arc<Mutex<HashMap<u64, EncoderCallback>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let keyframe_requested = Arc::new(AtomicBool::new(false));

            let latest_frame_tx_clone = latest_frame_tx.clone();
            let pw_listener = PipewireListener::new(
                self.pw_channel.clone(),
                pw_id,
                Box::new(move || {let _ = latest_frame_tx_clone.try_send(());}),
            );

            let callbacks_clone = callbacks.clone();
            let keyframe_clone = keyframe_requested.clone();
            let streams_clone = self.pw_streams.clone();
            let encoder_thread = thread::spawn(move || {
                if let Err(e) = encoder_thread_inner(
                    streams_clone,
                    pw_id,
                    latest_frame_rx,
                    callbacks_clone,
                    keyframe_clone,
                    EncoderOptions::default(),
                ) {
                    eprintln!("encoder {pw_id} failed: {e:#}");
                }
            });

            EncoderInner {
                callbacks,
                latest_frame_tx,
                keyframe_requested,
                _pw_listener: pw_listener,
                _encoder_thread: encoder_thread,
            }
        });

        inner.callbacks.lock().unwrap().insert(encoder_id, callback);

        Ok(EncoderReference {
            registry: self.encoders.clone(),
            pw_id,
            encoder_id,
        })
    }
}

pub struct EncoderReference {
    registry: Arc<Mutex<HashMap<PipewireID, EncoderInner>>>,
    pw_id: PipewireID,
    encoder_id: u64,
}

impl Drop for EncoderReference {
    fn drop(&mut self) {
        let mut map = self.registry.lock().unwrap();
        if let Some(inner) = map.get_mut(&self.pw_id) {
            inner.callbacks.lock().unwrap().remove(&self.encoder_id);
            if inner.callbacks.lock().unwrap().is_empty() {
                map.remove(&self.pw_id);
            }
        }
    }
}

/// Encoder thread: receives frame notifications, reads latest frame from stream, encodes to H.264.
/// Runs on a dedicated thread to avoid blocking PipeWire or WebRTC.
fn encoder_thread_inner(
    streams: Arc<StdRwLock<HashMap<PipewireID, Arc<StdRwLock<PipewireStream>>>>>,
    pw_id: PipewireID,
    frame_rx: mpsc::Receiver<FrameSignal>,
    callbacks: Arc<Mutex<HashMap<u64, EncoderCallback>>>,
    keyframe_requested: Arc<AtomicBool>,
    mut opts: EncoderOptions,
) -> Result<()> {
    opts.bitrate_bps = 4_000_000;
    let mut session: Option<HwEncodeSession> = None;
    let mut last_err = std::time::Instant::now() - Duration::from_secs(10);

    while frame_rx.recv().is_ok() {
        // println!("A");
        // Read latest frame from PipeWire stream state
        let frame = {
            let map = streams.read().ok().context("Streams read failed")?;
            let stream = map.get(&pw_id).context("Pw map not exists")?.read().ok().context("Failed to read")?;
            stream.latest_frame.context("No latest frame")?
        };

        let mut force_idr = keyframe_requested.swap(false, Ordering::Relaxed);

        // Recreate session if resolution changed
        if !session
            .as_ref()
            .is_some_and(|s| s.width == frame.width && s.height == frame.height)
        {
            session.take();
            match HwEncodeSession::new(&opts, frame.width, frame.height, frame.offset, frame.stride)
            {
                Ok(s) => {
                    session = Some(s);
                    force_idr = true;
                }
                Err(e) => {
                    if last_err.elapsed() > Duration::from_secs(5) {
                        eprintln!("encoder hw setup failed: {e:#}");
                        last_err = std::time::Instant::now();
                    }
                    continue;
                }
            }
        }

        let Some(sess) = session.as_mut() else {
            continue;
        };

        // Encode the frame (fd consumed by FrameData::drop in encode)
        match sess.encode(frame, force_idr) {
            Ok(packets) => {
                for (data, pts, is_keyframe) in packets {
                    let mut cbs = callbacks.lock().unwrap();
                    for cb in cbs.values_mut() {
                        cb(&data, pts, is_keyframe);
                    }
                }
            }
            Err(e) => {
                if last_err.elapsed() > Duration::from_secs(5) {
                    eprintln!("encode error: {e:#}");
                    last_err = std::time::Instant::now();
                }
                session = None;
            }
        }
    }
    Ok(())
}

// ============================================================================
// FFmpeg hardware encode session
// ============================================================================
// Zero-copy GPU encoding pipeline:
// dmabuf fd -> AVFrame(DRM_PRIME) -> av_hwframe_map (VA surface) ->
// scale_vaapi (BGR0->NV12 on GPU) -> h264_vaapi -> Annex-B packets
// All processing stays on GPU (VA-API), no CPU copies.
// ============================================================================

// RAII wrapper for AVBufferRef - manages reference counting
struct HwBufferRef(*mut ffi::AVBufferRef);
impl HwBufferRef {
    fn get(&self) -> *mut ffi::AVBufferRef {
        self.0
    }
    fn ref_clone(&self) -> Result<*mut ffi::AVBufferRef> {
        let r = unsafe { ffi::av_buffer_ref(self.0) };
        if r.is_null() {
            Err(anyhow::anyhow!("av_buffer_ref failed"))
        } else {
            Ok(r)
        }
    }
}
impl Drop for HwBufferRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::av_buffer_unref(&mut self.0) };
        }
    }
}

// RAII wrapper for AVFilterGraph - frees all filters on drop
struct FilterGraph(*mut ffi::AVFilterGraph);
impl Drop for FilterGraph {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::avfilter_graph_free(&mut self.0) };
        }
    }
}

/// Complete hardware encode session for one resolution.
/// Owns: VA device, frames context, filter graph, H.264 encoder.
/// All FFmpeg state is confined to the encoder thread (Send via unsafe impl).
struct HwEncodeSession {
    backend: HwBackend,
    width: u32,
    height: u32,
    offset: u32,
    stride: i32,
    next_pts: i64,
    encoder: codec::encoder::Video,
    graph: FilterGraph,
    src_ctx: *mut ffi::AVFilterContext,
    sink_ctx: *mut ffi::AVFilterContext,
    frames_ref: HwBufferRef,
    device_ref: HwBufferRef,
}
unsafe impl Send for HwEncodeSession {}

impl HwEncodeSession {
    /// Creates a new encode session for the given frame dimensions.
    /// Sets up: VA device -> frames context -> filter graph -> encoder.
    fn new(
    opts: &EncoderOptions,
    width: u32,
    height: u32,
    offset: u32,
    stride: i32,
) -> Result<Self> {
    let backend = opts.backend;

    let device_cstr = opts
        .device
        .as_ref()
        .map(|p| CString::new(p.as_os_str().as_encoded_bytes()))
        .transpose()?;

    // --- 1. Open hardware device ---
    let mut raw_dev: *mut ffi::AVBufferRef = ptr::null_mut();

    chk(
        unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut raw_dev,
                backend.device_type(),
                device_cstr
                    .as_ref()
                    .map(|s| s.as_ptr())
                    .unwrap_or(ptr::null()),
                ptr::null_mut(),
                0,
            )
        },
        "hw device",
    )?;

    let device_ref = HwBufferRef(raw_dev);

    // --- 2. Create frames context for imported dmabuf surfaces ---
    let raw_frames = unsafe { ffi::av_hwframe_ctx_alloc(device_ref.get()) };
    let frames_ref = HwBufferRef(raw_frames);

    if frames_ref.0.is_null() {
        return Err(anyhow::anyhow!("av_hwframe_ctx_alloc failed"));
    }

    unsafe {
        let frames_ctx = (*raw_frames).data as *mut ffi::AVHWFramesContext;

        (*frames_ctx).format = backend.hw_pix_fmt().into();

        // This must match the format described by the DRM PRIME frame.
        // (*frames_ctx).sw_format = Pixel::BGRZ.into();
        (*frames_ctx).sw_format = Pixel::BGRZ.into();

        (*frames_ctx).width = width as i32;
        (*frames_ctx).height = height as i32;

        // Surfaces are imported on demand from dmabufs.
        (*frames_ctx).initial_pool_size = 0;

        chk(
            ffi::av_hwframe_ctx_init(frames_ref.get()),
            "frames ctx init",
        )?;
    }

    // --- 3. Build filter graph ---
    let graph = FilterGraph(unsafe { ffi::avfilter_graph_alloc() });

    if graph.0.is_null() {
        return Err(anyhow::anyhow!("avfilter_graph_alloc failed"));
    }

    // Keep the hardware pixel format here. The source filter is initialized
    // only after hw_frames_ctx has been attached below.
    let src_args = CString::new(format!(
        "video_size={width}x{height}:pix_fmt={}:time_base=1/{RTP_CLOCK_RATE}:pixel_aspect=1/1",
        backend.pix_fmt_name()
    ))?;

    let mut src_ctx = ptr::null_mut();
    let mut sink_ctx = ptr::null_mut();
    let mut convert_ctx = ptr::null_mut();

    unsafe {
        let buffersrc =
            ffi::avfilter_get_by_name(b"buffer\0".as_ptr() as *const i8);

        let buffersink =
            ffi::avfilter_get_by_name(b"buffersink\0".as_ptr() as *const i8);

        let (conv_name, conv_args) = backend.convert_filter();

        let converter_name = CString::new(conv_name)?;
        let converter =
            ffi::avfilter_get_by_name(converter_name.as_ptr());

        if buffersrc.is_null() || buffersink.is_null() || converter.is_null() {
            return Err(anyhow::anyhow!(
                "required ffmpeg filters missing"
            ));
        }

        // Important:
        //
        // Do not use avfilter_graph_create_filter() for the buffer source.
        // That function initializes the source immediately, before its
        // hw_frames_ctx can be installed.
        src_ctx = ffi::avfilter_graph_alloc_filter(
            graph.0,
            buffersrc,
            b"in\0".as_ptr() as *const i8,
        );

        if src_ctx.is_null() {
            return Err(anyhow::anyhow!(
                "avfilter_graph_alloc_filter(buffer) failed"
            ));
        }

        // Attach the hardware frames context before initializing the source.
        let params = ffi::av_buffersrc_parameters_alloc();

        if params.is_null() {
            return Err(anyhow::anyhow!(
                "av_buffersrc_parameters_alloc failed"
            ));
        }

        let mut frames_for_src = frames_ref.ref_clone()?;

        (*params).format =
            ffi::AVPixelFormat::from(backend.hw_pix_fmt()) as i32;

        (*params).width = width as i32;
        (*params).height = height as i32;

        (*params).time_base = ffi::AVRational {
            num: 1,
            den: RTP_CLOCK_RATE,
        };

        (*params).sample_aspect_ratio = ffi::AVRational {
            num: 1,
            den: 1,
        };

        (*params).hw_frames_ctx = frames_for_src;

        let ret = ffi::av_buffersrc_parameters_set(src_ctx, params);

        // av_buffersrc_parameters_set() takes its own reference.
        ffi::av_buffer_unref(&mut frames_for_src);

        // ffi::av_freep((params as *mut _ as *mut c_void));

        chk(ret, "buffersrc params")?;

        // Now initialize the source. At this point the source already has
        // hw_frames_ctx, so pix_fmt=vaapi is valid.
        chk(
            ffi::avfilter_init_str(src_ctx, src_args.as_ptr()),
            "buffer src",
        )?;

        // Conversion: scale_vaapi, for example:
        // ("scale_vaapi", "format=nv12")
        let convert_args = CString::new(conv_args)?;

        chk(
            ffi::avfilter_graph_create_filter(
                &mut convert_ctx,
                converter,
                b"convert\0".as_ptr() as *const i8,
                convert_args.as_ptr(),
                ptr::null_mut(),
                graph.0,
            ),
            "converter",
        )?;

        // Output buffer sink.
        chk(
            ffi::avfilter_graph_create_filter(
                &mut sink_ctx,
                buffersink,
                b"out\0".as_ptr() as *const i8,
                ptr::null(),
                ptr::null_mut(),
                graph.0,
            ),
            "buffer sink",
        )?;

        // Link:
        //
        // buffer -> scale_vaapi -> buffersink
        let mut ret = ffi::avfilter_link(src_ctx, 0, convert_ctx, 0);

        if ret >= 0 {
            ret = ffi::avfilter_link(convert_ctx, 0, sink_ctx, 0);
        }

        chk(ret, "link filters")?;

        chk(
            ffi::avfilter_graph_config(graph.0, ptr::null_mut()),
            "config graph",
        )?;
    }

    // --- 4. Open H.264 hardware encoder ---
    let encoder_frames_ref =
    create_encoder_frames_ctx(&device_ref, backend, width, height)?;

let encoder = open_encoder(
    backend,
    width,
    height,
    opts,
    &encoder_frames_ref,
)?;


    Ok(Self {
        backend,
        width,
        height,
        offset,
        stride,
        next_pts: 0,
        encoder,
        graph,
        src_ctx,
        sink_ctx,
        frames_ref,
        device_ref,
    })
}


    /// Encodes one captured dmabuf frame to H.264 Annex-B packets.
    /// Pipeline: dmabuf fd -> DRM_PRIME frame -> hwframe_map (VA surface) ->
    /// filter graph (scale_vaapi) -> h264_vaapi encoder -> packets.
    fn encode(&mut self, frame: DmaBufFrame, force_idr: bool) -> Result<Vec<(Vec<u8>, i64, bool)>> {
        // --- Step 1: Wrap dmabuf fd in DRM_PRIME AVFrame ---
        let mut src = unsafe { self.build_drm_frame(&frame)? };

        // --- Step 2: Map dmabuf to VA surface via hwframe_map (zero-copy) ---
        let mut dst = unsafe { ffi::av_frame_alloc() };
        if dst.is_null() {
            unsafe { ffi::av_frame_free(&mut src) };
            return Err(anyhow::anyhow!("av_frame_alloc failed"));
        }

        unsafe {
            (*dst).format = ffi::AVPixelFormat::from(self.backend.hw_pix_fmt()) as i32;
            (*dst).width = self.width as i32;
            (*dst).height = self.height as i32;
            match self.frames_ref.ref_clone() {
                Ok(r) => (*dst).hw_frames_ctx = r,
                Err(e) => {
                    ffi::av_frame_free(&mut dst);
                    ffi::av_frame_free(&mut src);
                    return Err(e);
                }
            }
            // Maps the dmabuf (src) to a VA surface (dst) - no copy, just aliasing
            let ret = ffi::av_hwframe_map(dst, src, ffi::AV_HWFRAME_MAP_READ as i32);
            if ret < 0 {
                ffi::av_frame_free(&mut dst);
                ffi::av_frame_free(&mut src);
                return Err(ffmpeg_next::Error::from(ret)).context("av_hwframe_map failed");
            }
            // Push VA surface into filter graph; KEEP_REF lets graph hold it while we drop ours
            let ret = ffi::av_buffersrc_add_frame_flags(
                self.src_ctx,
                dst,
                ffi::AV_BUFFERSRC_FLAG_KEEP_REF as i32,
            );
            ffi::av_frame_free(&mut dst);
            ffi::av_frame_free(&mut src);
            if ret < 0 {
                return Err(ffmpeg_next::Error::from(ret)).context("buffersrc add failed");
            }
        }

        // --- Step 3: Pull converted frames from filter graph and encode ---
        let mut out = Vec::new();
        loop {
            let mut converted = unsafe { ffi::av_frame_alloc() };
            if converted.is_null() {
                return Err(anyhow::anyhow!("av_frame_alloc failed"));
            }
            // Pull NV12 frame from buffersink (output of scale_vaapi)
            let ret = unsafe { ffi::av_buffersink_get_frame(self.sink_ctx, converted) };
            if ret == -(nix::errno::Errno::EAGAIN as i32) {
                unsafe { ffi::av_frame_free(&mut converted) };
                break;
            } else if ret < 0 {
                unsafe { ffi::av_frame_free(&mut converted) };
                return Err(ffmpeg_next::Error::from(ret)).context("buffersink read failed");
            }

            // Assign timestamps and force IDR if requested (PLI/FIR from WebRTC)
            let pts = self.next_pts;
            let is_keyframe = force_idr;
            unsafe {
                (*converted).pts = pts;
                self.next_pts += PTS_STEP;
                if force_idr {
                    (*converted).pict_type = ffi::AVPictureType::AV_PICTURE_TYPE_I;
                }
            }

            // Send to encoder and drain all packets for this frame
            let converted_frame = unsafe { frame::Video::wrap(converted) };
            self.encoder.send_frame(&converted_frame)?;

            loop {
                let mut packet = codec::packet::Packet::empty();
                match self.encoder.receive_packet(&mut packet) {
                    Ok(()) => {
                        if let Some(data) = packet.data() {
                            out.push((data.to_vec(), pts, is_keyframe));
                        }
                    }
                    Err(ffmpeg_next::Error::Other { errno })
                        if errno == nix::errno::Errno::EAGAIN as i32 =>
                    {
                        break;
                    }
                    Err(ffmpeg_next::Error::Eof) => break,
                    Err(err) => return Err(err.into()),
                }
            }
        }
        Ok(out)
    }

    /// Wraps a dmabuf fd in a DRM_PRIME AVFrame for zero-copy import.
    /// The fd and descriptor are freed by FFmpeg when the last buffer ref drops.
    unsafe fn build_drm_frame(&self, frame: &DmaBufFrame) -> Result<*mut ffi::AVFrame> {
        // Holder owns the dmabuf fd and descriptor; freed in release callback
        struct Holder {
            fd: RawFd,
            desc: Box<ffi::AVDRMFrameDescriptor>,
        }
        // Called by FFmpeg when the buffer refcount reaches zero
        unsafe extern "C" fn release(opaque: *mut c_void, _data: *mut u8) {
            drop(unsafe { Box::from_raw(opaque as *mut Holder) });
        }

        let (desc, holder, mut buf) = unsafe {
            // Build DRM frame descriptor for single-plane BGRx dmabuf
            let desc = Box::into_raw(Box::new(ffi::AVDRMFrameDescriptor {
                nb_objects: 1,
                objects: [
                    ffi::AVDRMObjectDescriptor {
                        fd: frame.fd as i32,
                        size: frame.offset as usize
                            + (frame.stride.unsigned_abs() as usize) * frame.height as usize,
                        format_modifier: DRM_FORMAT_MOD_LINEAR,
                    },
                    std::mem::zeroed(),
                    std::mem::zeroed(),
                    std::mem::zeroed(),
                ],
                nb_layers: 1,
                layers: [
                    ffi::AVDRMLayerDescriptor {
                        format: DRM_FORMAT_XRGB8888,
                        nb_planes: 1,
                        planes: [
                            ffi::AVDRMPlaneDescriptor {
                                object_index: 0,
                                offset: frame.offset as _,
                                pitch: frame.stride as _,
                            },
                            std::mem::zeroed(),
                            std::mem::zeroed(),
                            std::mem::zeroed(),
                        ],
                    },
                    std::mem::zeroed(),
                    std::mem::zeroed(),
                    std::mem::zeroed(),
                ],
            }));
            let holder = Box::into_raw(Box::new(Holder {
                fd: frame.fd as i32,
                desc: Box::from_raw(desc),
            }));
            let buf = ffi::av_buffer_create(
                desc as *mut u8,
                std::mem::size_of::<ffi::AVDRMFrameDescriptor>(),
                Some(release),
                holder as *mut c_void,
                0,
            );
            (desc, holder, buf)
        };
        if buf.is_null() {
            drop(unsafe { Box::from_raw(holder) });
            return Err(anyhow::anyhow!("av_buffer_create failed"));
        }

        let frame_ptr = unsafe { ffi::av_frame_alloc() };
        if frame_ptr.is_null() {
            unsafe { ffi::av_buffer_unref(&mut buf) };
            return Err(anyhow::anyhow!("av_frame_alloc failed"));
        }
        unsafe {
            (*frame_ptr).format = ffi::AVPixelFormat::from(Pixel::DRM_PRIME) as i32;
            (*frame_ptr).width = self.width as i32;
            (*frame_ptr).height = self.height as i32;
            (*frame_ptr).data[0] = desc as *mut u8;
            (*frame_ptr).buf[0] = buf;
        }
        Ok(frame_ptr)
    }
}

fn open_encoder(
    backend: HwBackend,
    width: u32,
    height: u32,
    opts: &EncoderOptions,
    encoder_frames_ref: &HwBufferRef,
) -> Result<codec::encoder::Video> {
    if opts.bitrate_bps == 0 {
        return Err(anyhow::anyhow!(
            "encoder bitrate must be greater than zero"
        ));
    }

    let codec = codec::encoder::find_by_name(backend.encoder_name())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "encoder {} not available",
                backend.encoder_name()
            )
        })?;

    let mut ctx = codec::context::Context::new()
        .encoder()
        .video()?;

    ctx.set_width(width);
    ctx.set_height(height);
    ctx.set_format(backend.hw_pix_fmt());
    ctx.set_time_base(Rational(1, RTP_CLOCK_RATE));
    ctx.set_max_b_frames(0);
    ctx.set_gop(1 << 20);

    // Set this through the public ffmpeg-next API.
    // The value is bits per second.
    ctx.set_bit_rate(opts.bitrate_bps as usize);

    unsafe {
        let raw = ctx.as_mut_ptr();

        // Keep the raw assignment as well, especially for rc_max_rate and
        // rc_buffer_size.
        (*raw).bit_rate = opts.bitrate_bps as i64;
        (*raw).rc_max_rate = opts.bitrate_bps as i64;
        (*raw).rc_buffer_size = (opts.bitrate_bps / 80) as i32;

        // Required by h264_vaapi to associate the encoder with a device.
        (*raw).hw_frames_ctx = encoder_frames_ref.ref_clone()?;
    }

    let mut dict = dictionary::Owned::new();

    dict.set("rc_mode", "CBR");
    dict.set("async_depth", "1");
    dict.set("profile", "constrained_baseline");

    if backend == HwBackend::Vaapi {
        dict.set("compression_level", "0");
    }

    if backend == HwBackend::Vulkan {
        dict.set("strict", "experimental");
    }

    dict.set("flags", "+global_header");
    dict.set("repeat_pps", "1");


    Ok(ctx.open_as_with(codec, dict)?)
}



fn create_encoder_frames_ctx(
    device_ref: &HwBufferRef,
    backend: HwBackend,
    width: u32,
    height: u32,
) -> Result<HwBufferRef> {
    let raw_frames = unsafe {
        ffi::av_hwframe_ctx_alloc(device_ref.get())
    };

    let frames_ref = HwBufferRef(raw_frames);

    if frames_ref.0.is_null() {
        return Err(anyhow::anyhow!(
            "av_hwframe_ctx_alloc for encoder failed"
        ));
    }

    unsafe {
        let frames_ctx =
            (*raw_frames).data as *mut ffi::AVHWFramesContext;

        (*frames_ctx).format = backend.hw_pix_fmt().into();

        // The filter graph produces NV12 for h264_vaapi.
        (*frames_ctx).sw_format = Pixel::NV12.into();

        (*frames_ctx).width = width as i32;
        (*frames_ctx).height = height as i32;

        (*frames_ctx).initial_pool_size = 0;

        chk(
            ffi::av_hwframe_ctx_init(frames_ref.get()),
            "encoder frames ctx init",
        )?;
    }

    Ok(frames_ref)
}
