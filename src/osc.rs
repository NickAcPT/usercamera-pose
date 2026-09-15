use std::sync::{Arc, RwLock};

use crate::pose::{CameraPose, SharedPoseState};
use vrchat_osc::{
    Error, VRChatOSC,
    models::{OscNode, OscRootNode, OscValue},
    rosc::{OscMessage, OscPacket, OscType},
};

const SERVICE_NAME: &str = "NickUserCameraPose";
pub(crate) const USER_CAMERA_POSE_ADDRESS: &str = "/usercamera/Pose";

pub(crate) const VRCHAT_CLIENT_SERVICE: &str = "VRChat-Client-*";

const USER_CAMERA_MODE_ADDRESS: &str = "/usercamera/Mode";
const USER_CAMERA_STREAMING_ADDRESS: &str = "/usercamera/Streaming";
const USER_CAMERA_STREAM_MODE: i32 = 2;

#[derive(Default)]
pub struct CameraStreamState {
    mode: RwLock<Option<i32>>,
    streaming: RwLock<Option<bool>>,
}

impl CameraStreamState {
    fn mode_is_stream(&self) -> bool {
        *self.mode.read().expect("camera mode lock poisoned") == Some(USER_CAMERA_STREAM_MODE)
    }

    fn streaming_is_enabled(&self) -> bool {
        *self
            .streaming
            .read()
            .expect("camera streaming lock poisoned")
            == Some(true)
    }

    fn update_mode(&self, mode: i32) {
        *self.mode.write().expect("camera mode lock poisoned") = Some(mode);
    }

    fn update_streaming(&self, streaming: bool) {
        *self
            .streaming
            .write()
            .expect("camera streaming lock poisoned") = Some(streaming);
    }

    fn update_from_node(&self, node: &OscNode) {
        if let Some(OscValue::Int(mode)) = node
            .contents
            .get("Mode")
            .and_then(|node| node.value.as_ref())
            .and_then(|value| value.first())
        {
            self.update_mode(*mode);
        }
        if let Some(OscValue::Bool(streaming)) = node
            .contents
            .get("Streaming")
            .and_then(|node| node.value.as_ref())
            .and_then(|value| value.first())
        {
            self.update_streaming(*streaming);
        }
    }
}

pub async fn initialize_camera_stream_state(
    vrchat_osc: &VRChatOSC,
    camera_stream_state: &CameraStreamState,
) -> Result<(), Error> {
    if let Some((_, node)) = vrchat_osc
        .get_parameter("/usercamera", VRCHAT_CLIENT_SERVICE)
        .await?
        .into_iter()
        .next()
    {
        camera_stream_state.update_from_node(&node);
    }
    Ok(())
}

pub async fn configure_camera_stream(
    vrchat_osc: &VRChatOSC,
    camera_stream_state: &CameraStreamState,
) -> Result<(), Error> {
    if !camera_stream_state.mode_is_stream() {
        send_usercamera_message(
            vrchat_osc,
            USER_CAMERA_MODE_ADDRESS,
            OscType::Int(USER_CAMERA_STREAM_MODE),
        )
        .await?;
    }
    if !camera_stream_state.streaming_is_enabled() {
        send_usercamera_message(
            vrchat_osc,
            USER_CAMERA_STREAMING_ADDRESS,
            OscType::Bool(true),
        )
        .await?;
    }
    Ok(())
}

async fn send_usercamera_message(
    vrchat_osc: &VRChatOSC,
    address: &str,
    argument: OscType,
) -> Result<(), Error> {
    vrchat_osc
        .send(
            OscPacket::Message(OscMessage {
                addr: address.to_owned(),
                args: vec![argument],
            }),
            VRCHAT_CLIENT_SERVICE,
        )
        .await
}

pub async fn send_camera_pose(
    vrchat_osc: &VRChatOSC,
    camera_pose: CameraPose,
) -> Result<(), Error> {
    vrchat_osc
        .send(
            OscPacket::Message(OscMessage {
                addr: USER_CAMERA_POSE_ADDRESS.to_owned(),
                args: camera_pose.0.into_iter().map(OscType::Float).collect(),
            }),
            VRCHAT_CLIENT_SERVICE,
        )
        .await
}

pub async fn register_usercamera_service(
    vrchat_osc: &VRChatOSC,
    pose_state: SharedPoseState,
    camera_stream_state: Arc<CameraStreamState>,
) -> Result<(), Error> {
    let root_node = OscRootNode::new().with_usercamera();
    vrchat_osc
        .register(SERVICE_NAME, root_node, move |packet| {
            let OscPacket::Message(message) = packet else {
                return;
            };
            match message.addr.as_str() {
                USER_CAMERA_POSE_ADDRESS => {
                    let camera_pose = match &message.args[..] {
                        [
                            OscType::Float(a),
                            OscType::Float(b),
                            OscType::Float(c),
                            OscType::Float(d),
                            OscType::Float(e),
                            OscType::Float(f),
                        ] => CameraPose([*a, *b, *c, *d, *e, *f]),
                        _ => {
                            log::error!("Unexpected OSC camera pose arguments: {:?}", message.args);
                            return;
                        }
                    };

                    pose_state.update(camera_pose);
                    log::debug!("Received OSC camera pose: {:?}", camera_pose.0);
                }
                USER_CAMERA_MODE_ADDRESS => match message.args.as_slice() {
                    [OscType::Int(mode)] => camera_stream_state.update_mode(*mode),
                    _ => log::error!("Unexpected OSC camera mode arguments: {:?}", message.args),
                },
                USER_CAMERA_STREAMING_ADDRESS => match message.args.as_slice() {
                    [OscType::Bool(streaming)] => camera_stream_state.update_streaming(*streaming),
                    _ => log::error!(
                        "Unexpected OSC camera streaming arguments: {:?}",
                        message.args
                    ),
                },
                _ => {}
            }
        })
        .await
}
