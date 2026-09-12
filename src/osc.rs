use crate::pose::{CameraPose, SharedPoseState};
use vrchat_osc::{
    models::OscRootNode,
    rosc::{OscPacket, OscType},
    Error, VRChatOSC,
};

const SERVICE_NAME: &str = "NickUserCameraPose";
const USER_CAMERA_POSE_ADDRESS: &str = "/usercamera/Pose";

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
