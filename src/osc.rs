use crate::pose::{CameraPose, SharedPoseState};
use vrchat_osc::{
    Error, VRChatOSC,
    models::OscRootNode,
    rosc::{OscMessage, OscPacket, OscType},
};

const SERVICE_NAME: &str = "NickUserCameraPose";
const USER_CAMERA_POSE_ADDRESS: &str = "/usercamera/Pose";

const VRCHAT_CLIENT_SERVICE: &str = "VRChat-Client-*";

const USER_CAMERA_MODE_ADDRESS: &str = "/usercamera/Mode";
const USER_CAMERA_STREAMING_ADDRESS: &str = "/usercamera/Streaming";
const USER_CAMERA_STREAM_MODE: i32 = 2;

pub async fn configure_camera_stream(vrchat_osc: &VRChatOSC) -> Result<(), Error> {
    send_usercamera_message(
        vrchat_osc,
        USER_CAMERA_MODE_ADDRESS,
        OscType::Int(USER_CAMERA_STREAM_MODE),
    )
    .await?;
    send_usercamera_message(
        vrchat_osc,
        USER_CAMERA_STREAMING_ADDRESS,
        OscType::Bool(true),
    )
    .await
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
) -> Result<(), Error> {
    let root_node = OscRootNode::new().with_usercamera();
    vrchat_osc
        .register(SERVICE_NAME, root_node, move |packet| {
            if let OscPacket::Message(message) = packet
                && message.addr == USER_CAMERA_POSE_ADDRESS
            {
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
        })
        .await
}
