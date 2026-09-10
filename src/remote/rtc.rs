use std::{sync::{Arc, Mutex, OnceLock}, time::Duration};

use anyhow::Context;
use eframe::egui::{self, ViewportId};
use tokio::sync::mpsc::UnboundedSender;
use webrtc::{data_channel::{DataChannel, DataChannelEvent}, media_stream::track_local::{TrackLocal, static_sample::TrackLocalStaticSample}, peer_connection::{MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, Registry, register_default_interceptors}, rtp_transceiver::RtpSender, runtime::{Runtime, Sender, default_runtime}};
use rtc::{data_channel::RTCDataChannelInit, media::Sample, media_stream::MediaStreamTrack, peer_connection::configuration::media_engine::{MIME_TYPE_H264, MIME_TYPE_RTX}, rtp::extension::{HeaderExtension, playout_delay_extension::PlayoutDelayExtension}, rtp_transceiver::{PayloadType, SSRC, rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RTCRtpHeaderExtensionCapability, RtpCodecKind}}};

use crate::{remote::{connection::{BindableVirtualDevice, handle_remote_message}, encoder::{EncoderReference, EncoderRegistry}, websocket::RemoteConnectionInner}, session::InstanceInputEvt};
use bytes::{self, Bytes};


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


 
const RTP_CLOCK_RATE: i32 = 90_000;

/// Signaled codec: constrained-baseline, packetization-mode 1. Matches the encoder's
/// constrained_baseline profile so every browser can decode it.
const H264_FMTP: &str = "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f";

/// A sending H.264 track bound to the peer connection, plus its SSRC/sender handles.
pub struct ClientVideoTrack {
    pub track: Arc<TrackLocalStaticSample>,
    pub sender: Arc<dyn RtpSender>,
    pub ssrc: SSRC,
}

/// Creates an H.264 `TrackLocalStaticSample` and adds it to the peer connection.
/// Must be called before the SDP answer is produced so the video m-line lands in the answer.
async fn create_h264_video_track<P: PeerConnection + ?Sized>(
    pc: &P,
    stream_id: &str,
) -> anyhow::Result<ClientVideoTrack> {
    let ssrc = fastrand::u32(..) | 1;
    let codec = RTCRtpCodec {
        mime_type: rtc::peer_connection::configuration::media_engine::MIME_TYPE_H264.to_owned(),
        clock_rate: RTP_CLOCK_RATE as u32,
        channels: 0,
        sdp_fmtp_line: H264_FMTP.to_owned(),
        rtcp_feedback: vec![],
    };

    let track = Arc::new(TrackLocalStaticSample::new(MediaStreamTrack::new(
        stream_id.to_owned(),
        format!("partydeck-video-{ssrc}"),
        "gamescope-capture".to_owned(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec,
            ..Default::default()
        }],
    )).context("Creating new track")?);

    let sender = pc.add_track(track.clone() as Arc<dyn TrackLocal>).await.context("Adding track - If you get this while on firefox, please enable h264 extentions!")?;
    println!("Added track!");

    Ok(ClientVideoTrack {
        track,
        sender,
        ssrc,
    })
}

/// Async glue: pushes encoded access units onto the RTP track with the playout-delay header
/// extension (min = max = 0: play out as soon as decoded) stamped on every packet.
async fn writer_task(
    video: ClientVideoTrack,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Bytes>,
) {
    let mut h264_payload_type: Option<PayloadType> = None;

    while let Some(data) = rx.recv().await {
        if h264_payload_type.is_none() {
            h264_payload_type = video
                .sender
                .get_parameters()
                .await
                .ok()
                .and_then(|p| {
                    // Find H.264 codec that matches your encoder's actual profile
                    p.rtp_parameters.codecs.iter()
                        .find(|c| {
                            c.rtp_codec.mime_type == rtc::peer_connection::configuration::media_engine::MIME_TYPE_H264
                        })
                        .map(|c| c.payload_type)
                });
        }

        let sample = Sample {
            data: data,
            duration: Duration::from_millis(33), // ~30fps - doesnt matter at all.
            timestamp: rtc::shared::time::SystemInstant::now(),
            ..Default::default()
        };

        let result = video
            .track
            .sample_writer(video.ssrc, h264_payload_type.unwrap())
            .with_extension(HeaderExtension::PlayoutDelay(
                PlayoutDelayExtension::new(0, 0),
            ))
            .write_sample(&sample)
            .await;

        if let Err(err) = result {
            eprintln!("rtp write failed: {err}");
        }
    }
}


async fn create_peer_connection(runtime: Arc<dyn Runtime>, handler: Arc<TestHandler>) -> anyhow::Result<impl PeerConnection> {

    let mut media_engine = MediaEngine::default();
    // Instead of this single line but that adds like 30 lines of every answer, I just hard code the one I use.
    // Most of this code is extracted from this method:
    // media_engine.register_default_codecs()?;

    let video_rtcp_feedback = vec![
        RTCPFeedback {
            typ: "goog-remb".to_owned(),
            parameter: "".to_owned(),
        },
        RTCPFeedback {
            typ: "ccm".to_owned(),
            parameter: "fir".to_owned(),
        },
        RTCPFeedback {
            typ: "nack".to_owned(),
            parameter: "".to_owned(),
        },
        RTCPFeedback {
            typ: "nack".to_owned(),
            parameter: "pli".to_owned(),
        },
    ];
    
    let codec = RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: RTP_CLOCK_RATE as u32,
            channels: 0,
            sdp_fmtp_line:
                H264_FMTP.to_owned(),
            rtcp_feedback: video_rtcp_feedback.clone(),
        },
        payload_type: 108,
    };
    let rtx_codec = |payload_type: PayloadType, apt: PayloadType| RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_RTX.to_owned(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: format!("apt={apt}"),
            rtcp_feedback: vec![],
        },
        payload_type,
    };
    media_engine.register_codec(codec, RtpCodecKind::Video)?;
    media_engine.register_codec(rtx_codec(109, 108), RtpCodecKind::Video)?;


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
                "stun:stun.nextcloud.com:443".to_string()
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

    Ok(pc)
}


pub struct RemoteClient {
    connected: bool,
    #[allow(unused)]
    id: String,
    pub con_inner: Arc<Mutex<RemoteConnectionInner>>
}
impl RemoteClient {
    pub fn new(offer: String, id: String, msg_resp: UnboundedSender<String>, con_inner: Arc<Mutex<RemoteConnectionInner>>, encoder: Arc<Mutex<EncoderRegistry>>, egui_ctx: egui::Context) -> Arc<Mutex<Self>> {

        let ret_self = Arc::new(Mutex::new(Self { connected: false, id: id.clone(), con_inner }));
        let self_arc = ret_self.clone();
        
        tokio::spawn(async move {
            Self::inner(self_arc, offer, id, msg_resp, encoder, egui_ctx).await.unwrap();
        });

        ret_self
    }

    async fn inner(self_arc: Arc<Mutex<Self>>, offer_sdp: String, ws_id: String, msg_resp: UnboundedSender<String>, encoder: Arc<Mutex<EncoderRegistry>>, egui_ctx: egui::Context) -> anyhow::Result<()> {
        let (done_tx, mut done_rx) = webrtc::runtime::channel::<RTCPeerConnectionState>(1);
        let (gather_complete_tx, mut gather_complete_rx) = webrtc::runtime::channel(1);


        let runtime = runtime();


        let handler = Arc::new(TestHandler {
            gather_complete_tx,
            done_tx,
        });

        let pc = create_peer_connection(runtime.clone(), handler).await.context("Creating peer connection")?;
        

        pc.set_remote_description(serde_json::from_str(&offer_sdp)?).await?;


        let data_channel = pc.create_data_channel("control_channel", Some(RTCDataChannelInit {negotiated: Some(42),..Default::default()})).await?;
        
        let video_track =
            create_h264_video_track(&pc, "partydeck-video").await.context("Making h264 video track")?;
        

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

        let (video_packet_tx, video_packet_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
        runtime.clone().spawn(Box::pin(writer_task(video_track, video_packet_rx)));

        
        Self::main_message_loop(
            self_arc.clone(), 
            data_channel, 
            done_rx, 
            encoder, 
            video_packet_tx,
            &egui_ctx
        ).await.context("Main message processing loop")?; // Not sure if this is a good idea being fully ran 24/7 but IDRC


        self_arc.lock().unwrap().connected = false;

        Ok(())
    }

    async fn main_message_loop(
        self_arc: Arc<Mutex<Self>>, 
        dc: Arc<dyn DataChannel>, 
        mut done_rx: webrtc::runtime::Receiver<RTCPeerConnectionState>, 
        encoder: Arc<Mutex<EncoderRegistry>>, 
        video_packet_tx: UnboundedSender<Bytes>,
        egui_ctx: &egui::Context
    ) -> anyhow::Result<()> {
        let mut opt_uinput_dev = BindableVirtualDevice::new().await.inspect_err(|e| eprintln!("Failed to create uinput virtual controller: {e:?}")).ok();
        let mut selected_instance: Option<(u64, Arc<Mutex<Vec<InstanceInputEvt>>>, Option<EncoderReference>, ViewportId)> = None;

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
                    let msg_str = str::from_utf8(&msg.data).context("FAILED TO UTF8 DECODE")?;
                    let msg_parsed = serde_json::from_str(msg_str).with_context(|| format!("FAILED TO DECODE SERDE MESSAGE: {msg_str}"))?;

                    let resp_opt = handle_remote_message(
                        msg_parsed,

                        &self_arc,
                        &encoder,
                        &video_packet_tx,
                        &egui_ctx,
                        
                        &mut opt_uinput_dev,
                        &mut selected_instance
                    ).await.with_context(|| format!("Executing requested command: {msg_str}"))?;
                    

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
}

