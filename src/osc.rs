use crate::pose::{CameraPose, SharedPoseState};
use vrchat_osc::{
    models::OscRootNode,
    rosc::{OscMessage, OscPacket, OscType},
    Error, VRChatOSC,
};

const SERVICE_NAME: &str = "NickUserCameraPose";
const USER_CAMERA_POSE_ADDRESS: &str = "/usercamera/Pose";

const VRCHAT_CLIENT_SERVICE: &str = "VRChat-Client-*";

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
                    [OscType::Float(a), OscType::Float(b), OscType::Float(c), OscType::Float(d), OscType::Float(e), OscType::Float(f)] => CameraPose([*a, *b, *c, *d, *e, *f]),
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
