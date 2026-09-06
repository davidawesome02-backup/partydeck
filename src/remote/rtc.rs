use std::{path::PathBuf, sync::{Arc, Mutex, OnceLock}, time::Duration};

use anyhow::Context;
use evdev::{AbsInfo, AbsoluteAxisCode, BusType, InputId, KeyCode};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use webrtc::{data_channel::{DataChannel, DataChannelEvent}, peer_connection::{MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, Registry, register_default_interceptors}, runtime::{Runtime, Sender, default_runtime}};
use rtc::{data_channel::RTCDataChannelInit, rtp_transceiver::rtp_sender::{RTCRtpHeaderExtensionCapability, RtpCodecKind}};

use crate::{remote::{encoder::EncoderRegistry, shared::RemoteConnectionInner}, session::InstanceInputEvt};
use crate::video::pipewire::PipewireID;


/// TODO: temporary target until the client protocol requests real streams.
/// PipeWire object id of the gamescope node every viewer gets for now.
const HARDCODED_STREAM_PW_ID: PipewireID = 67;


static RUNTIME: OnceLock<Arc<dyn Runtime>> = OnceLock::new();
pub fn runtime() -> Arc<dyn Runtime> {
    Arc::clone(RUNTIME.get_or_init(|| {
        default_runtime().expect("no runtime feature enabled (runtime-tokio or runtime-smol)")
    }))
}


#[derive(Clone)]
struct TestHandler {
    gather_complete_tx: Sender<()>,
    done_tx: Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for TestHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        println!("ICE gathering state: {:?}", state);
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_complete_tx.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        println!("Peer Connection State has changed: {state}");
        if state == RTCPeerConnectionState::Failed || state == RTCPeerConnectionState::Closed || state == RTCPeerConnectionState::Connected {
            let _ = self.done_tx.try_send(state);
        }
    }

    async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
        println!("Offered data channel: {:?}", data_channel.label().await)
    }
}


pub struct RemoteClient {
    connected: bool,
    id: String,
    con_inner: Arc<Mutex<RemoteConnectionInner>>
}
impl RemoteClient {
    pub fn new(offer: String, id: String, msg_resp: UnboundedSender<String>, con_inner: Arc<Mutex<RemoteConnectionInner>>, encoder: Arc<Mutex<EncoderRegistry>>) -> Arc<Mutex<Self>> {

        let ret_self = Arc::new(Mutex::new(Self { connected: false, id: id.clone(), con_inner }));
        let self_arc = ret_self.clone();
        
        tokio::spawn(async move {
            Self::inner(self_arc, offer, id, msg_resp, encoder).await.unwrap();
        });

        ret_self
    }

    async fn inner(self_arc: Arc<Mutex<Self>>, offer_sdp: String, ws_id: String, msg_resp: UnboundedSender<String>, encoder: Arc<Mutex<EncoderRegistry>>) -> anyhow::Result<()> {
        let (done_tx, mut done_rx) = webrtc::runtime::channel::<RTCPeerConnectionState>(1);
        let (gather_complete_tx, mut gather_complete_rx) = webrtc::runtime::channel(1);

        let runtime = runtime();

        let handler = Arc::new(TestHandler {
            gather_complete_tx,
            done_tx,
        });

        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;


        const PLAYOUT_DELAY_URI: &str = "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay";

        media_engine
            .register_header_extension(
                RTCRtpHeaderExtensionCapability {
                    uri: PLAYOUT_DELAY_URI.to_string(),
                },
                RtpCodecKind::Video,
                None,
            )
            .expect("register playout delay extension");
        media_engine
            .register_header_extension(
                RTCRtpHeaderExtensionCapability {
                    uri: PLAYOUT_DELAY_URI.to_string(),
                },
                RtpCodecKind::Audio,
                None,
            )
            .expect("register playout delay extension");


        let registry = Registry::new();
        // Use the default set of Interceptors
        let registry = register_default_interceptors(registry, &mut media_engine)?;

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer {
                urls: vec![
                    "stun:stun.l.google.com:19302".to_string(),
                    "stun:stun1.l.google.com:19302".to_string()
                ],
                ..Default::default()
            }, RTCIceServer {
                urls: vec![
                    "stun:stun.services.mozilla.com:3478".to_string()
                ],
                ..Default::default()
            }])
            .build();

        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_runtime(runtime.clone())
            .with_udp_addrs(vec![format!("0.0.0.0:0")])
            .build()
            .await?;

        

        // Create a datachannel with label 'data'
        // data_channel.id
        // std::mem::forget(data_channel); // TODO DONT DO THIS
        // tokio::spawn(async move {
        //     Self::message_handler(self_arc, data_channel).await.unwrap();
        // });

        // Attach our outgoing video track before answering so the video m-line lands in the
        // SDP answer (adding it afterwards would require renegotiation).
        // let video_track =
        //     crate::remote::encoder::create_h264_video_track(&pc, "partydeck-video").await?;

        pc.set_remote_description(serde_json::from_str(&offer_sdp)?).await?;


        // let data_channel_REMOVE = pc.create_data_channel("AAAA", None).await?;
        // std::mem::forget(data_channel_REMOVE);
        // let data_channel = pc.create_data_channel("control_channel", None).await?;
        let data_channel = pc.create_data_channel("control_channel", Some(RTCDataChannelInit {negotiated: Some(42),..Default::default()})).await?;



        let answer = pc.create_answer(None).await?;
        pc.set_local_description(answer).await?;

        // Wait for ICE gathering to complete (non-trickle)
        gather_complete_rx.recv().await;

        let answer_sdp = pc
            .local_description()
            .await
            .ok_or_else(|| anyhow::anyhow!("no local description"))?;

        let json_ans_str = serde_json::to_string(&answer_sdp)?;
        
        
        msg_resp.send(serde_json::json!({ // May rename to answer later, but for now was "response" because could be failure
            "type": "response",
            "client_id": ws_id,
            "response": json_ans_str
        }).to_string())?;

        let connection_timeout = tokio::time::sleep(tokio::time::Duration::from_secs(60));
        tokio::select! {
            _ = connection_timeout => {
                println!("Connection timeout: no OnOpen after 60 seconds");
                return Ok(());
            },
            started_type = done_rx.recv() => {
                match started_type {
                    Some(RTCPeerConnectionState::Connected ) => {},
                    None | Some(_) => {
                        println!("Connection failed :(");
                        return Ok(())
                    },
                }
            }
        }

        self_arc.lock().unwrap().connected = true;


        std::mem::forget(encoder.lock().unwrap().listen(91, Box::new(|a: &[u8], b: i64, c: bool| {
            println!("{a:#?} {b} {c}");
        })).unwrap());


       
        // listen

        // let data_channel_REMOVE = pc.create_data_channel("BBBB", None).await?;
        // println!("POLL TEST");
        // println!("POLL TEST B {:?}", data_channel_REMOVE.poll().await);
        // std::mem::forget(data_channel_REMOVE);

        // pc.
        Self::main_message_loop(self_arc.clone(), data_channel, done_rx).await.context("Main message processing loop")?; // Not sure if this is a good idea being fully ran 24/7 but IDRC

        // drop(video_encoder);

        self_arc.lock().unwrap().connected = false;

        Ok(())
    }

    async fn main_message_loop(self_arc: Arc<Mutex<Self>>, dc: Arc<dyn DataChannel>, mut done_rx: webrtc::runtime::Receiver<RTCPeerConnectionState>) -> anyhow::Result<()> {
        let mut opt_uinput_dev = Self::create_uinput_if_possible().await.inspect_err(|e| eprintln!("Failed to create uinput virtual controller: {e:?}")).ok();
        let mut selected_instance: Option<(u64, Arc<Mutex<Vec<InstanceInputEvt>>>)> = None;

        loop {
            // println!("Waiting on datachannel message!");
            let datachannel_poll_data = {
                tokio::select! {
                    msg_type = done_rx.recv() => {
                        if matches!(msg_type, Some(RTCPeerConnectionState::Connected)) {continue;}
                        None
                    },
                    data_channel_msg = dc.poll() => data_channel_msg
                }
            };

            match datachannel_poll_data {
                Some(DataChannelEvent::OnMessage(msg)) => {
                    let msg_str = str::from_utf8(&msg.data).context("FAILED TO UTF8 DECODE")?;
                    let msg_parsed = serde_json::from_str(msg_str).with_context(|| format!("FAILED TO DECODE SERDE MESSAGE: {msg_str}"))?;

                    
                    let resp_opt = match msg_parsed {
                        RTCClientMessage::Status => {
                            Self::handle_status_msg(self_arc.clone(), msg_parsed, opt_uinput_dev.is_some())
                        },
                        RTCClientMessage::Select { id: new_instance_id } => { // I hate this code, todo replace.
                            if 
                                let Some(arc_running_session_data) = &self_arc.lock().unwrap().con_inner.lock().unwrap().session_data
                            {
                                let mut running_session_data = arc_running_session_data.lock().unwrap();
                                
                                if 
                                    let Some(ref selected_instance) = selected_instance &&
                                    let Some(ref uinput_dev) = opt_uinput_dev 
                                {                                    
                                    selected_instance.1.lock().unwrap().push(
                                        InstanceInputEvt::RemoveDev(
                                            uinput_dev.1.to_string_lossy().trim_start_matches("/dev/input/").to_string()
                                        )
                                    );
                                }

                                if let Some(new_instance_id) = new_instance_id {
                                    let instance = running_session_data.displays.iter_mut().find_map(|d| {
                                        d.instances.iter_mut().find(|i| {
                                            i.id.0 == new_instance_id
                                        })
                                    });

                                    if 
                                        let Some(instance) = instance &&
                                        let Some(launched_instance) = &mut instance.launch_data
                                    {
                                        selected_instance = Some((new_instance_id, launched_instance.input_events.clone()));
                                        
                                        if let Some(ref uinput_dev) = opt_uinput_dev {
                                            launched_instance.input_events.lock().unwrap().push(
                                                InstanceInputEvt::AddDev(
                                                    uinput_dev.1.to_string_lossy().trim_start_matches("/dev/input/").to_string()
                                                )
                                            );
                                        }
                                        
                                    } else {
                                        eprintln!("Failed to setup remote device to user");
                                    selected_instance = None;
                                    }
                                } else {
                                    selected_instance = None;
                                }
                            }
                            

                            // TODO update the ffmpeg stream we are listening to too.

                            Ok(None)
                        },
                        RTCClientMessage::RemoteInput { input_type, input_code, input_value } => { // Keyboard OR mouse button
                            let constructed_event =
                                evdev::InputEvent::new(
                                    input_type,
                                    input_code,
                                    input_value
                                );

                            if
                                let Some(ref mut virt_dev) = opt_uinput_dev &&
                                ((
                                    input_type == evdev::EventType::KEY.0 && 
                                    PARTY_DECK_REMOTE_CONTROLLER_BUTTONS.contains(&KeyCode::new(input_code))
                                ) || (
                                    input_type == evdev::EventType::ABSOLUTE.0 && 
                                    PARTY_DECK_REMOTE_CONTROLLER_AXIS.iter().any(
                                        |a| a.0.0 == input_code
                                    )
                                ))
                            {
                                // Handle any valid controller events. We only support those we bound already.
                                virt_dev.0.emit(&[constructed_event])?;
                            } else if let Some(ref instance_data) = selected_instance {
                                // Handle mouse and keyboard input or just drop it internally at this point. This lets the main system handle anything else.
                                instance_data.1.lock().unwrap().push(InstanceInputEvt::InputEvt(constructed_event));
                            }

                            Ok(None)
                        },
                        // _ => {Ok(None)}
                    }.with_context(|| format!("Executing requested command: {msg_str}"))?;


                    if let Some(resp) = resp_opt {
                        dc.send_text(&resp).await.context("Sening response")?;
                    }
                },
                None | Some(DataChannelEvent::OnClose) => {
                    println!("WEBRTC connection closed!");
                    return Ok(());
                }
                _ => {},
            }
        }
    }

    fn handle_status_msg(self_arc: Arc<Mutex<Self>>, _msg_parsed: RTCClientMessage, has_controller_support: bool) -> anyhow::Result<Option<String>> {

        
        let session_data = { // Avoid locking for too long.
            let client_self = self_arc.lock().unwrap();
            let shared_self = client_self.con_inner.lock().unwrap();
            shared_self.session_data.clone()
        };

        let message_to_send = match session_data {
            Some(session_data) => {
                // Lock contention hell. Please fix in the future // TODO
                let mut session_data = session_data.lock().unwrap();
                let encoded_instance = session_data.displays.iter_mut().flat_map(|d| {
                    d.instances.iter_mut().map(|i| {
                        ServerInstanceInfo {
                            id: i.id.0,
                            name: i.profname.clone(),
                            color: i.color.to_hex(),
                            alive: i.is_alive_or_starting(),
                        }
                        // i.launch_data.map(|ld| {
                        //     ld.
                        // })
                    })
                }).collect();

                RTCServerMessage::StatusStarted { instances: encoded_instance, has_controller_support }
            },
            None => RTCServerMessage::StatusNotStarted
        };

        Ok(Some(serde_json::to_string(&message_to_send)?))
    }

    async fn create_uinput_if_possible() -> anyhow::Result<(evdev::uinput::VirtualDevice, PathBuf)> {
        let mut dev_builder = evdev::uinput::VirtualDevice::builder().context("Cant open uinput")?
            .name(&"Partydeck Virtual Remote Controller")
            .input_id(InputId::new(BusType::BUS_USB, 12, 12, 1))
            .with_keys(&evdev::AttributeSet::from_iter(PARTY_DECK_REMOTE_CONTROLLER_BUTTONS)).context("Invalid keys")?;

        for axis in PARTY_DECK_REMOTE_CONTROLLER_AXIS {
            dev_builder = dev_builder.with_absolute_axis(&evdev::UinputAbsSetup::new(axis.0, AbsInfo::new(0, axis.1, axis.2, 0, 0, 0))).context("Invalid Axis")?;
        }

        let mut dev = dev_builder.build().context("Failed to build")?;
        tokio::time::sleep(Duration::from_millis(500)).await; // The kernel scares me. Just give it time to calm down.
        let mut path_checker = dev.enumerate_dev_nodes_blocking().context("Failed to open device node")?;
        let path_dev = path_checker.next().ok_or(anyhow::anyhow!("No path found"))?.context("No path found")?;

        println!("New virtual device added for RTC: {path_dev:?}");
        Ok((dev, path_dev))
    }
}



#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum RTCClientMessage {
    #[serde(rename = "status")]
    Status,

    #[serde(rename = "select")]
    Select {id: Option<u64>},

    #[serde(rename = "remote_input")]
    RemoteInput {input_type: u16, input_code: u16, input_value: i32}
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum RTCServerMessage {
    #[serde(rename = "status_not_started")]
    StatusNotStarted,

    #[serde(rename = "status_started")]
    StatusStarted { instances: Vec<ServerInstanceInfo>, has_controller_support: bool },
}

#[derive(Serialize, Deserialize, Debug)]
struct ServerInstanceInfo {
    id: u64,
    name: String,
    color: String,
    alive: bool
}

static PARTY_DECK_REMOTE_CONTROLLER_BUTTONS: [KeyCode; 17] = [ // Used as a bitmask, only append to work with new protos.
    KeyCode::BTN_SOUTH, // A
    KeyCode::BTN_EAST,  // B
    KeyCode::BTN_WEST,  // X
    KeyCode::BTN_NORTH, // Y

    KeyCode::BTN_TL,     // LB
    KeyCode::BTN_TR,     // RB

    KeyCode::BTN_TL2, // Take a wild guess
    KeyCode::BTN_TR2, // Take a wild guess

    KeyCode::BTN_SELECT, // Back / View
    KeyCode::BTN_START,  // Menu / Start

    KeyCode::BTN_THUMBL, // Left stick click
    KeyCode::BTN_THUMBR, // Right stick click
    
    KeyCode::BTN_DPAD_UP,
    KeyCode::BTN_DPAD_DOWN,
    KeyCode::BTN_DPAD_LEFT,
    KeyCode::BTN_DPAD_RIGHT,

    KeyCode::BTN_MODE   // Xbox button
];

static PARTY_DECK_REMOTE_CONTROLLER_AXIS: [(AbsoluteAxisCode, i32, i32); 4] = [
    (AbsoluteAxisCode::ABS_X, -32768, 32767),
    (AbsoluteAxisCode::ABS_Y, -32768, 32767),
    (AbsoluteAxisCode::ABS_RX, -32768, 32767),
    (AbsoluteAxisCode::ABS_RY, -32768, 32767),
    // (AbsoluteAxisCode::ABS_Z, 0, 255), // Left trigger
    // (AbsoluteAxisCode::ABS_RZ, 0, 255), // Right trigger
];

