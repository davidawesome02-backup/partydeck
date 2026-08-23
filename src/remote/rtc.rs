use std::{path::PathBuf, sync::{Arc, Mutex, OnceLock}, time::Duration};

use anyhow::Context;
use eframe::egui::Color32;
use evdev::{BusType, InputId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use webrtc::{data_channel::{DataChannel, DataChannelEvent}, peer_connection::{MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, Registry, register_default_interceptors}, runtime::{Runtime, Sender, default_runtime}};
use rtc::rtp_transceiver::rtp_sender::{RTCRtpHeaderExtensionCapability, RtpCodecKind};

use crate::remote::shared::RemoteConnectionInner;


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
}


pub struct RemoteClient {
    connected: bool,
    id: String,
    con_inner: Arc<Mutex<RemoteConnectionInner>>
}
impl RemoteClient {
    pub fn new(offer: String, id: String, msg_resp: UnboundedSender<String>, con_inner: Arc<Mutex<RemoteConnectionInner>>) -> Arc<Mutex<Self>> {

        let ret_self = Arc::new(Mutex::new(Self { connected: false, id: id.clone(), con_inner  }));
        let self_arc = ret_self.clone();
        
        tokio::spawn(async move {
            Self::inner(self_arc, offer, id, msg_resp).await.unwrap();
        });

        ret_self
    }

    async fn inner(self_arc: Arc<Mutex<Self>>, offer_sdp: String, ws_id: String, msg_resp: UnboundedSender<String>) -> anyhow::Result<()> {
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
        let data_channel = pc.create_data_channel("data", None).await?;
        // std::mem::forget(data_channel); // TODO DONT DO THIS
        // tokio::spawn(async move {
        //     Self::message_handler(self_arc, data_channel).await.unwrap();
        // });


        pc.set_remote_description(serde_json::from_str(&offer_sdp)?).await?;
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

        Self::main_message_loop(self_arc.clone(), data_channel, done_rx).await?; // Not sure if this is a good idea being fully ran 24/7 but IDRC

        self_arc.lock().unwrap().connected = false;

        Ok(())
    }

    async fn main_message_loop(self_arc: Arc<Mutex<Self>>, dc: Arc<dyn DataChannel>, mut done_rx: webrtc::runtime::Receiver<RTCPeerConnectionState>) -> anyhow::Result<()> {
        let mut opt_dev = Self::create_uinput_if_possible().await.inspect_err(|e| eprintln!("Failed to create uinput virtual controller: {e:?}")).ok();
        let mut selected_instance: Option<u64>;

        loop {
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
                    let msg_str = str::from_utf8(&msg.data)?;
                    let msg_parsed = serde_json::from_str(msg_str)?;
                    
                    let resp_opt = match msg_parsed {
                        RTCClientMessage::Status => {
                            Self::handle_status_msg(self_arc.clone(), msg_parsed)
                        },
                        _ => {Ok(None)}
                    }?;


                    if let Some(resp) = resp_opt {
                        dc.send_text(&resp).await?;                        
                    }
                },
                Some(_) => {},
                None => {
                    println!("WEBRTC connection closed!");
                    return Ok(());
                }
            }
        }
    }

    fn handle_status_msg(self_arc: Arc<Mutex<Self>>, _msg_parsed: RTCClientMessage) -> anyhow::Result<Option<String>> {
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
                            color: i.color.to_array(),
                            alive: i.is_alive_or_starting(),
                        }
                        // i.launch_data.map(|ld| {
                        //     ld.
                        // })
                    })
                }).collect();

                RTCServerMessage::StatusStarted { instances: encoded_instance }
            },
            None => RTCServerMessage::StatusNotStarted
        };

        Ok(Some(serde_json::to_string(&message_to_send)?))
    }

    async fn create_uinput_if_possible() -> anyhow::Result<(evdev::uinput::VirtualDevice, PathBuf)> {
        let dev_builder = evdev::uinput::VirtualDevice::builder().context("Cant open uinput")?
            // .with_(&evdev::AttributeSet::from_iter([evdev::EventType::KEY])).context("Invalid keys")?
            .name(&"hi")
            .input_id(InputId::new(BusType::BUS_USB, 12, 12, 1))
            .with_keys(&evdev::AttributeSet::from_iter([evdev::KeyCode::KEY_SPACE; 1])).context("Invalid keys")?;

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
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum RTCServerMessage {
    #[serde(rename = "status_not_started")]
    StatusNotStarted,

    #[serde(rename = "status_started")]
    StatusStarted { instances: Vec<ServerInstanceInfo> },
}

#[derive(Serialize, Deserialize, Debug)]
struct ServerInstanceInfo {
    id: u64,
    name: String,
    color: [u8; 4],
    alive: bool
}