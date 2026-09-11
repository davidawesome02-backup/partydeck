use std::{collections::HashMap, str::FromStr, sync::{Arc, Mutex}, thread::{self, JoinHandle}, time::Duration};

use eframe::egui;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{select, sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel}};
use tokio_tungstenite::{self, tungstenite};

use crate::{app::toasts, remote::{encoder::EncoderRegistry, rtc::RemoteClient}, session::Session};

type WsType = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[allow(unused)]
pub enum RemoteCommand {
    Connect,
    Disconnect,
    // KickAll,
    Terminate,
}

pub enum WebsocketConnectionStatus {
    Connected,
    Disconnected,
    Connecting,
    Disconnecting,
} 

pub struct RemoteConnection {
    pub channel: UnboundedSender<RemoteCommand>,
    #[allow(unused)]
    thread: JoinHandle<()>,
    pub inner: Arc<Mutex<RemoteConnectionInner>>,
}
impl RemoteConnection {
    pub fn new(encoder: Arc<Mutex<EncoderRegistry>>, egui_ctx: egui::Context, toasts: toasts::Toasts) -> Result<Self, std::io::Error> {

        let (send, rec) = unbounded_channel();
        
        let con_inner = Arc::new(Mutex::new(
            RemoteConnectionInner{ session_data: None, code: None, status: WebsocketConnectionStatus::Disconnected, clients: HashMap::new(), encoder, egui_ctx }
        ));
        let con_inner_clone = con_inner.clone();
        
        let thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build().unwrap();
            rt.block_on(async {
                RemoteConnectionInner::inner(con_inner, rec, toasts).await.unwrap()
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
    pub status: WebsocketConnectionStatus,
    clients: HashMap<String, Arc<Mutex<RemoteClient>>>,
    encoder: Arc<Mutex<EncoderRegistry>>,
    egui_ctx: egui::Context,
}

impl RemoteConnectionInner {
    pub async fn inner(self_arc: Arc<Mutex<Self>>, mut rec: UnboundedReceiver<RemoteCommand>, toasts: toasts::Toasts) -> anyhow::Result<()> {
        let mut ws: Option<WsType> = None;
        let (send_ws_send_request, mut rec_ws_send_request) = unbounded_channel();
        loop {
            let mut disconnect_ws = false;
            select! {
                msg = rec.recv() => match msg {
                    Some(RemoteCommand::Connect) => {
                        {
                            let mut self_ = self_arc.lock().unwrap();
                            self_.status = WebsocketConnectionStatus::Connecting;
                        }
                        let mut url = url::Url::from_str("http://localhost:8788").unwrap();
                        url.set_scheme("ws").expect("Failed to set protocol");
                        url.set_path("/websocket");

                        let duration = Duration::from_secs(15);

                        match tokio::time::timeout(duration, tokio_tungstenite::connect_async(url)).await {
                            Ok(Ok((ws_stream, _response))) => {
                                println!("Connected to WS successfully!");
                                ws = Some(ws_stream);
                                let mut self_ = self_arc.lock().unwrap();
                                self_.status = WebsocketConnectionStatus::Connected;
                                toasts.push(toasts::Severity::Info, "Remote connection", "WebSocket connection established!");
                            }
                            Ok(Err(e)) => {
                                {
                                    let mut self_ = self_arc.lock().unwrap();
                                    self_.status = WebsocketConnectionStatus::Disconnected;
                                }
                                toasts.push(toasts::Severity::Info, "Remote failed", "WebSocket connection error");

                                eprintln!("WebSocket connection error: {e}");
                            }
                            Err(_) => {
                                {
                                    let mut self_ = self_arc.lock().unwrap();
                                    self_.status = WebsocketConnectionStatus::Disconnected;
                                }
                                toasts.push(toasts::Severity::Info, "Remote failed", "Connection timed out after 15 seconds");

                                eprintln!("Connection timed out after 15 seconds");
                            }
                        }
                    },
                    Some(RemoteCommand::Disconnect) => {
                        disconnect_ws = true;
                        {
                            let mut self_ = self_arc.lock().unwrap();
                            self_.status = WebsocketConnectionStatus::Disconnecting;
                        }
                        // toasts.push(toasts::Severity::Info, "Remote disconnect", "Disconnected from remote websocket.\nClients may still be connected");
                    },
                    Some(RemoteCommand::Terminate) => {
                        return Ok(());
                    },
                    // Some(RemoteCommand::KickAll) => {
                    //     let mut self_ = self_arc.lock().unwrap();
                    //     self_.clients.drain();
                    // }
                    None => {},
                },
                ws_msg = async {if let Some(ref mut ws) = ws {ws.next().await} else {std::future::pending().await}} => {
                    match ws_msg {
                        Some(Ok(m)) => {
                            disconnect_ws = Self::parse_ws_message(self_arc.clone(), m, send_ws_send_request.clone(), &toasts).await?;
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
                self_.status = WebsocketConnectionStatus::Disconnected;
                self_.code = None;
                println!("Disconnected from websocket!");
                toasts.push(toasts::Severity::Info, "Remote disconnected", "WebSocket connection closed!");
            }
        }
    }

    async fn parse_ws_message(self_arc: Arc<Mutex<Self>>, msg: tungstenite::Message, msg_resp: UnboundedSender<String>, toasts: &toasts::Toasts) -> anyhow::Result<bool> {
        
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
                let egui_ctx_clone = self_.egui_ctx.clone();
                self_.clients.insert(
                    client_id.clone(),
                    RemoteClient::new(offer, client_id, msg_resp, self_arc.clone(), encoder_clone, egui_ctx_clone, toasts.clone())
                );
            },
            
            WsMessage::ClientError { client_error, client_id } => todo!("{client_error}, {client_id}"),
        }

        Ok(false)
    }

    // because called externally, I will assume they will borrow it, not me.
    pub fn get_alive_clients(&self) -> usize {
        self.clients.iter().filter(
            |c| c.1.lock().unwrap().connected
        ).count()
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

