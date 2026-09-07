use evdev::{AbsInfo, AbsoluteAxisCode, BusType, InputId, KeyCode};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::{Arc, Mutex}, time::Duration};

use anyhow::Context;
use tokio::sync::mpsc::UnboundedSender;

use crate::{remote::{encoder::{EncoderReference, EncoderRegistry}, rtc::RemoteClient}, session::InstanceInputEvt};
use bytes::{self, Bytes};


pub async fn handle_remote_message(
    msg_parsed: RTCClientMessage,

    self_arc: &Arc<Mutex<RemoteClient>>,
    encoder: &Arc<Mutex<EncoderRegistry>>, 
    video_packet_tx: &UnboundedSender<Bytes>,

    opt_uinput_dev: &mut Option<(evdev::uinput::VirtualDevice, PathBuf)>,
    selected_instance: &mut Option<(u64, Arc<Mutex<Vec<InstanceInputEvt>>>, Option<EncoderReference>)>
) -> anyhow::Result<Option<String>> {
    match msg_parsed {
        RTCClientMessage::Status => {
            handle_status_msg(self_arc.clone(), msg_parsed, opt_uinput_dev.is_some())
        },
        RTCClientMessage::Select { id: new_instance_id } => { // I hate this code, todo replace.
            if 
                let Some(arc_running_session_data) = &self_arc.lock().unwrap().con_inner.lock().unwrap().session_data
            {
                let mut running_session_data = arc_running_session_data.lock().unwrap();
                
                if 
                    let &mut Some(ref selected_instance) = selected_instance &&
                    let &mut Some(ref uinput_dev) = opt_uinput_dev 
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

                        let new_encoder_ref = { // TODO make this not terrible
                            let tmp_encoder_clone = encoder.clone();
                            let tmp_video_packet_tx_clone = video_packet_tx.clone();
                            launched_instance.stream_view.as_ref().and_then(move |sv| {
                                tmp_encoder_clone.lock().unwrap().listen(sv.pipewire_node, Box::new(move |data: &[u8], _: i64, _: bool| {
                                    let _ = tmp_video_packet_tx_clone.send(Bytes::copy_from_slice(data));
                                })).inspect_err(|e| eprintln!("Failed to create encoder for view: {e:#?}")).ok()
                            })
                        };

                        *selected_instance = Some((
                            new_instance_id, 
                            launched_instance.input_events.clone(), 
                            new_encoder_ref
                        ));
                        
                        if let &mut Some(ref uinput_dev) = opt_uinput_dev {
                            launched_instance.input_events.lock().unwrap().push(
                                InstanceInputEvt::AddDev(
                                    uinput_dev.1.to_string_lossy().trim_start_matches("/dev/input/").to_string()
                                )
                            );
                        }
                        
                    } else {
                        eprintln!("Failed to setup remote device to user");
                        *selected_instance = None;
                    }
                } else {
                    *selected_instance = None;
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
                let &mut Some(ref mut virt_dev) = opt_uinput_dev &&
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
            } else if let &mut Some(ref mut instance_data) = selected_instance {
                // Handle mouse and keyboard input or just drop it internally at this point. This lets the main system handle anything else.
                instance_data.1.lock().unwrap().push(InstanceInputEvt::InputEvt(constructed_event));
            }

            Ok(None)
        },
    }
}


fn handle_status_msg(self_arc: Arc<Mutex<RemoteClient>>, _msg_parsed: RTCClientMessage, has_controller_support: bool) -> anyhow::Result<Option<String>> {
    
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
                })
            }).collect();

            RTCServerMessage::StatusStarted { instances: encoded_instance, has_controller_support }
        },
        None => RTCServerMessage::StatusNotStarted
    };

    Ok(Some(serde_json::to_string(&message_to_send)?))
}

pub async fn create_uinput_if_possible() -> anyhow::Result<(evdev::uinput::VirtualDevice, PathBuf)> {
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



#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum RTCClientMessage {
    #[serde(rename = "status")]
    Status,

    #[serde(rename = "select")]
    Select {id: Option<u64>},

    #[serde(rename = "remote_input")]
    RemoteInput {input_type: u16, input_code: u16, input_value: i32}
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum RTCServerMessage {
    #[serde(rename = "status_not_started")]
    StatusNotStarted,

    #[serde(rename = "status_started")]
    StatusStarted { instances: Vec<ServerInstanceInfo>, has_controller_support: bool },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ServerInstanceInfo {
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

