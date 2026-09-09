use std::{collections::HashMap, sync::{Arc, Mutex, RwLock, atomic::AtomicU64}, thread::JoinHandle};

use pipewire::{self as pw, context::ContextRc, core::CoreRc, main_loop::MainLoopRc, stream::{StreamListener, StreamRc}};
use pw::spa;
use spa::pod::Pod;

use crate::video::pipewire::PipewireCommand::Disconnect;

static LISTENER_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub type PipewireID = u32;

pub struct PipewireListener {
    ch: pw::channel::Sender<PipewireCommand>,
    listen_id: u64,
    pw_id: u32,
}

pub type PwCBtype = Box<dyn FnMut() + Send + Sync + 'static>;

impl PipewireListener {
    pub fn new(ch: pw::channel::Sender<PipewireCommand>, pw_id: PipewireID, cb: PwCBtype) -> Self {
        let listen_id = LISTENER_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let _ = ch.send(PipewireCommand::ConnectVid(listen_id, pw_id, cb));
        
        Self { ch, listen_id, pw_id }
    }
    pub fn pw_id(&self) -> u32 {
        self.pw_id
    }
}
impl Drop for PipewireListener {
    fn drop(&mut self) {
        let _ = self.ch.send(Disconnect(self.listen_id, self.pw_id));
    }
}

pub enum PipewireCommand {
    ConnectVid(u64, PipewireID, PwCBtype),
    Disconnect(u64, PipewireID),
    Terminate
}
pub struct PipewireInstance {
    pub channel: pw::channel::Sender<PipewireCommand>,
    pub _thread: JoinHandle<()>,
    pub streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
}


impl PipewireInstance {
    pub fn new() -> Result<Self, std::io::Error> {
        let (sender, receiver) = pw::channel::channel::<PipewireCommand>();

        let streams = Arc::new(RwLock::new(HashMap::new()));

        let streams_clone = streams.clone();

        let thread = std::thread::Builder::new()
            .name("pipewire-thread".into())
            .spawn(move || {
                if let Err(err) = pipewire_thread_inner(streams.clone(), receiver) {
                    eprintln!("Pipewire thread creation error: {err}");
                }
            })?;
            
        let new_instance = PipewireInstance { channel: sender, _thread: thread, streams: streams_clone };

        return Ok(new_instance)
    }
}

impl Drop for PipewireInstance {
    fn drop(&mut self) {
        let _ = self.channel.send(PipewireCommand::Terminate);
    }
}

fn pipewire_thread_inner(streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>, receiver: pipewire::channel::Receiver<PipewireCommand>) -> Result<(), pw::Error> {
    pw::init();

    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let loop_ref = mainloop.clone();

    // Keep them alive!
    let listener_hashmap: RwLock<HashMap<PipewireID, (StreamListener<Arc<RwLock<PipewireStream>>>, StreamRc)>> = RwLock::new(HashMap::new());

    let _attached = receiver.attach(mainloop.loop_(), move |cmd| match cmd {
        PipewireCommand::ConnectVid(listen_id, pw_id, cb) => {
            println!("Connecting to pipewire stream: {pw_id}");

            let Ok(mut streams_map) = streams.write() else {
                eprintln!("Connect: Pipewire response stream map poisoned!");
                return;
            };

            if !streams_map.contains_key(&pw_id) {
                let Ok(new_pw_obj) = 
                    PipewireStream::new(pw_id, loop_ref.clone(), context.clone(), core.clone())
                    .inspect_err(|e| eprintln!("Error from pipewire connect: {e}")) else {return};

                streams_map.insert(pw_id, new_pw_obj.0);

                if let Ok(mut listener_map_write) = listener_hashmap.write() {
                    listener_map_write.insert(pw_id, new_pw_obj.1);
                } else {
                    eprintln!("Connect: Pipewire listener map poisoned!");
                }
            }

            let stream = streams_map.get(&pw_id).unwrap().read().unwrap();
            let mut listeners = stream.listeners.lock().unwrap();
            listeners.insert(listen_id, cb);
        }
        PipewireCommand::Disconnect(listen_id, pw_id) => {
            let Ok(mut listen_hashmap_lock) = listener_hashmap.write() else {
                eprintln!("Disconnect: Pipewire listener map poisoned!");
                return;
            };

            let Ok(mut streams_map) = streams.write() else {
                eprintln!("Disconnect: Pipewire response stream map poisoned!");
                return;
            };

            let Some(stream) = streams_map.get(&pw_id) else {
                eprintln!("No stream to remove");
                return;
            };
            
            let Ok(stream_loc) = stream.read() else {
                eprintln!("Disconnect: Pipewire response stream poisoned!");
                return;
            };

            let mut listeners = stream_loc.listeners.lock().unwrap();
            listeners.remove(&listen_id);

            if listeners.len() == 0 {
                std::mem::drop(listeners); // Needed to inform the compiler that we can free our ref to streams_map
                std::mem::drop(stream_loc); // Needed to inform the compiler that we can free our ref to streams_map

                if listen_hashmap_lock.remove(&pw_id).is_none() {
                    eprintln!("No stream to disconenct listener ({pw_id})"); 
                    return;
                }

                // Cant disconnect directly because storing the RC causes multi-thread issues,
                // Probably should just aquire a writer on main thread to write this, because we cant do true error handling here.

                let Some(stream_removed) = streams_map.remove(&pw_id) else {
                    eprintln!("No stream to disconenct ({pw_id})"); 
                    return;
                };

                let Ok(mut stream_removed_writer) = stream_removed.write() else {
                    eprintln!("Failed to disconnect stream properly: {pw_id}");
                    return;
                };

                stream_removed_writer.streaming = false;
            }

        }
        PipewireCommand::Terminate => {
            loop_ref.quit();
        }
    });

    mainloop.run();

    Ok(())
}


/// One imported DMA-BUF frame, as delivered by the process callback.
#[derive(Clone, Copy)]
pub struct DmaBufFrame {
    pub seq: u64,
    pub fd: i64,
    pub width: u32,
    pub height: u32,
    pub offset: u32,
    pub stride: i32,
}

pub struct PipewireStream {
    pub id: PipewireID,
    pub streaming: bool,

    pub latest_frame: Option<DmaBufFrame>,

    pub spa_format_latest: spa::param::video::VideoInfoRaw,

    listeners: Arc<Mutex<HashMap<u64, PwCBtype>>>,
}

impl PipewireStream {
    fn new(
        pw_id_target: PipewireID,
        _mainloop: MainLoopRc,
        _context: ContextRc,
        core: CoreRc
    ) -> Result<(Arc<RwLock<PipewireStream>>, (StreamListener<Arc<RwLock<PipewireStream>>>, StreamRc)), Box<dyn std::error::Error>> {
        let stream = StreamRc::new(
            core,
            "gamescope-dmabuf-capture",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",

                // Numeric fallback because this is all you currently have.
                "node.target" => pw_id_target.to_string(),

                "node.dont-fallback" => "true",
                "node.dont-reconnect" => "true",
            },
        )?;

        let new_stream_metadata = Arc::new(RwLock::new(
            PipewireStream {
                id: pw_id_target,
                streaming: false,

                latest_frame: None,

                spa_format_latest: Default::default(),

                listeners: Arc::new(Mutex::new(HashMap::new())),
            }
        ));


        let stream_metadata_clone = new_stream_metadata.clone();


        let _listener = stream
            .add_local_listener_with_user_data(stream_metadata_clone)
            .state_changed(move |_stream, stream_metadata, _old, new| {
                use pw::stream::StreamState;
                // println!("STATE CHANGED! {new:?}");
                

                let mut write_pw_stream = stream_metadata.write().unwrap(); 
                write_pw_stream.streaming = matches!(new, StreamState::Streaming);
                if write_pw_stream.streaming {
                    write_pw_stream.latest_frame = None;
                }
            })
            .param_changed(move |stream: &pipewire::stream::Stream, stream_metadata, fmt_id, param| {
                // TODO watch out, if this gets called after the stream is dropped, we may have been moved off our target ID.
                // println!("PARAM CHANGED!");
                let Some(param) = param else { return };
                if fmt_id != spa::param::ParamType::Format.as_raw() {
                    return;
                }

                let Ok((media_type, media_subtype)) =
                    spa::param::format_utils::parse_format(param)
                else {
                    return;
                };
                if media_type != spa::param::format::MediaType::Video
                    || media_subtype != spa::param::format::MediaSubtype::Raw
                {
                    return;
                }

                let mut write_pw_stream = stream_metadata.write().unwrap();
                if write_pw_stream.id != pw_id_target {
                    write_pw_stream.latest_frame = None;
                    println!("Erased stream when it was tried to be moved");
                    return;
                } // Only connect to our ID, DONT FALLBACK

                if write_pw_stream.spa_format_latest.parse(param).is_err() {
                    return;
                }

                // println!("PARAM CHANGED TO PASSING VALUE!! {} - {}, {:?}, {:?}, {:?}", write_pw_stream.id, pw_id_target, write_pw_stream.spa_format_latest, media_type, media_subtype);


                // Reply with our buffer requirements, asking for DMA-BUF memory.
                let buffers = build_buffers_param();
                if let Some(pod) = Pod::from_bytes(&buffers) {
                    let mut params = [pod];
                    let _ = stream.update_params(&mut params);
                }

                
            })
            .process(move |stream: &pipewire::stream::Stream, stream_metadata| {
                // Drain to the newest available buffer; reassigning `newest` drops
                // (and thereby re-queues) the previous one.
                let mut newest = None;
                while let Some(buf) = stream.dequeue_buffer() {
                    newest = Some(buf);
                }
                let Some(mut buffer) = newest else { return };

                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }
                let data = &mut datas[0];

                // Only handle DMA-BUF memory; anything else means negotiation did
                // not give us the zero-copy path and we skip the frame.
                if data.type_().as_raw() != spa::sys::SPA_DATA_DmaBuf {
                    return;
                }

                let raw = data.as_raw();
                let fd = raw.fd;
                if fd < 0 {
                    return;
                }

                let chunk = data.chunk();
                let offset = chunk.offset();
                let stride = chunk.stride();
                if chunk.size() == 0 {
                    // No new content this cycle.
                    return;
                }

                let mut write_pw_stream = stream_metadata.write().unwrap();

                let size = write_pw_stream.spa_format_latest.size();
                let seq = write_pw_stream.latest_frame.map_or(0, |frame| frame.seq) + 1;
                write_pw_stream.latest_frame = Some(DmaBufFrame {
                    seq,
                    fd,
                    width: size.width,
                    height: size.height,
                    offset,
                    stride,
                });

                // Get before drop, but drop so the funcs can lock us again to get the frame data.
                let listeners = write_pw_stream.listeners.clone(); 

                drop(write_pw_stream);

                for listener in listeners.lock().unwrap().values_mut() {
                    listener();
                }
            })
            .register()?;


        let values = build_enum_format();
        let Some(pod) = Pod::from_bytes(&values) else {
            return Err("Failed to build EnumFormat pod".into());
        };

        let mut params = [pod];
        stream.connect(
            spa::utils::Direction::Input,
            Some(pw_id_target),
            pw::stream::StreamFlags::AUTOCONNECT,
            &mut params,
        )?;

        Ok((new_stream_metadata, (_listener, stream)))
    }
}






/// Serialize a POD `Value` to bytes.
fn serialize_pod(value: &spa::pod::Value) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value)
        .expect("POD serialization cannot fail for a well-formed value")
        .0
        .into_inner()
}

/// Build the EnumFormat filter: BGRx, LINEAR modifier (mandatory on gamescope's
/// side), permissive size and framerate ranges.
fn build_enum_format() -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    use spa::param::ParamType;
    use spa::utils::{Fraction, Rectangle, SpaTypes};

    let obj = spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRx),
        // Presence of a modifier property is what makes gamescope export a
        // DMA-BUF. LINEAR (== 0) is the only layout it offers.
        spa::pod::property!(FormatProperties::VideoModifier, Long, 0_i64),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 60, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );

    serialize_pod(&spa::pod::Value::Object(obj))
}

/// Build the Buffers param reply that requests DMA-BUF memory (single plane).
///
/// The buffer-param property keys are raw `spa_sys` enum constants (no
/// `as_raw()` wrapper), so this is constructed directly rather than via the
/// `property!` macro.
fn build_buffers_param() -> Vec<u8> {
    use spa::pod::{Object, Property, PropertyFlags, Value};
    use spa::param::ParamType;
    use spa::utils::SpaTypes;

    let datatype_mask = 1i32 << (spa::sys::SPA_DATA_DmaBuf as i32);

    let obj = Value::Object(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            Property {
                key: spa::sys::SPA_PARAM_BUFFERS_buffers,
                flags: PropertyFlags::empty(),
                value: Value::Int(4),
            },
            Property {
                key: spa::sys::SPA_PARAM_BUFFERS_blocks,
                flags: PropertyFlags::empty(),
                // Single plane (BGRx is one plane).
                value: Value::Int(1),
            },
            Property {
                key: spa::sys::SPA_PARAM_BUFFERS_dataType,
                flags: PropertyFlags::empty(),
                value: Value::Int(datatype_mask),
            },
        ],
    });

    serialize_pod(&obj)
}
