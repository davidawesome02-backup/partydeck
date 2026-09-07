use std::{collections::HashMap, str::FromStr, sync::{Arc, Mutex}, thread::{self, JoinHandle}, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{select, sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel}};
use tokio_tungstenite::{self, tungstenite};

use crate::{remote::{encoder::EncoderRegistry, rtc::RemoteClient}, session::Session, video::pipewire::PipewireID};

type ws_type = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;


pub enum RemoteCommand {
    Connect,
    Disconnect,
    SetSessionData(Option<Arc<Mutex<Session>>>),
    Terminate,
}

pub struct RemoteConnection {
    pub channel: UnboundedSender<RemoteCommand>,
    thread: JoinHandle<()>,
    pub inner: Arc<Mutex<RemoteConnectionInner>>,
}
impl RemoteConnection {
    pub fn new(encoder: Arc<Mutex<EncoderRegistry>>) -> Result<Self, std::io::Error> {

        let (send, rec) = unbounded_channel();
        
        let con_inner = Arc::new(Mutex::new(
            RemoteConnectionInner{ session_data: None, code: None, connected: false, clients: HashMap::new(), encoder }
        ));
        let con_inner_clone = con_inner.clone();
        
        let thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build().unwrap();
            rt.block_on(async {
                RemoteConnectionInner::inner(con_inner, rec).await.unwrap()
            });
        });

        Ok(Self { thread, inner: con_inner_clone.clone(), channel: send })
    }

    pub fn update_session_data(&mut self, session_data: Option<Arc<Mutex<Session>>>) {
        self.inner.lock().unwrap().session_data = session_data;
    }
}

pub struct RemoteConnectionInner {
    pub session_data: Option<Arc<Mutex<Session>>>,
    pub code: Option<String>,
    pub connected: bool,
    clients: HashMap<String, Arc<Mutex<RemoteClient>>>,
    encoder: Arc<Mutex<EncoderRegistry>>,
}

impl RemoteConnectionInner {
    pub async fn inner(self_arc: Arc<Mutex<Self>>, mut rec: UnboundedReceiver<RemoteCommand>) -> anyhow::Result<()> {
        let mut ws: Option<ws_type> = None;
        let (send_ws_send_request, mut rec_ws_send_request) = unbounded_channel();
        loop {
            let mut disconnect_ws = false;
            select! {
                msg = rec.recv() => match msg {
                    Some(RemoteCommand::Connect) => {
                        let mut url = url::Url::from_str("http://localhost:8788").unwrap();
                        url.set_scheme("ws").expect("Failed to set protocol");
                        url.set_path("/websocket");

                        let duration = Duration::from_secs(15);

                        match tokio::time::timeout(duration, tokio_tungstenite::connect_async(url)).await {
                            Ok(Ok((ws_stream, _response))) => {
                                println!("Connected to WS successfully!");
                                ws = Some(ws_stream);
                                let mut self_ = self_arc.lock().unwrap();
                                self_.connected = true;
                            }
                            Ok(Err(e)) => {
                                eprintln!("WebSocket connection error: {e}");
                            }
                            Err(_) => {
                                eprintln!("Connection timed out after 15 seconds");
                            }
                        }
                    },
                    Some(RemoteCommand::Disconnect) => {
                        disconnect_ws = true;
                    },
                    Some(RemoteCommand::SetSessionData(session_data)) => {
                        let mut self_ = self_arc.lock().unwrap();
                        self_.session_data = session_data;
                    },
                    Some(RemoteCommand::Terminate) => {
                        return Ok(());
                    },
                    None => {},
                },
                ws_msg = async {if let Some(ref mut ws) = ws {ws.next().await} else {std::future::pending().await}} => {
                    match ws_msg {
                        Some(Ok(m)) => {
                            disconnect_ws = Self::parse_ws_message(self_arc.clone(), m, send_ws_send_request.clone()).await?;
                        },
                        Some(Err(e)) => {
                            eprintln!("WS Error: {}", e);
                            disconnect_ws = true;
                        },
                        None => disconnect_ws = true
                    }
                }
                ws_requested_send_msg = rec_ws_send_request.recv() => {
                    let Some(ref mut ws) = ws else {continue};
                    let Some(msg) = ws_requested_send_msg else {continue};
                    println!("{:?}", msg);
                    ws.send(msg.into()).await?;
                }
            }

            if disconnect_ws {
                if let Some(ref mut stream) = ws {
                    let _ = stream.close(None).await;
                }

                ws = None;
                
                let mut self_ = self_arc.lock().unwrap();
                self_.connected = false;
                self_.code = None;
            }
        }
    }

    async fn parse_ws_message(self_arc: Arc<Mutex<Self>>, msg: tungstenite::Message, msg_resp: UnboundedSender<String>) -> anyhow::Result<bool> {
        
        let tungstenite::Message::Text(ws_text_msg) = msg else {return Ok(false)};
        let txt_string = ws_text_msg.to_string();

        let parsed_msg: WsMessage = serde_json::from_str(&txt_string)?;
        println!("{:?}", parsed_msg);
        match parsed_msg {
            WsMessage::Code { code } => {
                let mut self_ = self_arc.lock().unwrap();
                self_.code = Some(code);
            },
            WsMessage::Error { error } => {
                eprintln!("WS error: {error}");
                return Ok(true)
            },
            WsMessage::Offer { offer, client_id } => {
                let mut self_ = self_arc.lock().unwrap();
                let encoder_clone = self_.encoder.clone();
                self_.clients.insert(
                    client_id.clone(),
                    RemoteClient::new(offer, client_id, msg_resp, self_arc.clone(), encoder_clone)
                );
            },
            WsMessage::ClientError { client_error, client_id } => todo!(),
        }

        Ok(false)
    }
}



#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum WsMessage {
    #[serde(rename = "code")]
    Code { code: String },
    
    #[serde(rename = "error")]
    Error { error: String },

    #[serde(rename = "offer")]
    Offer { offer: String, client_id: String },

    #[serde(rename = "client_error")]
    ClientError { client_error: String, client_id: String },
}

