use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use utoipa::ToSchema;

const UPDATE_BUFFER_CAPACITY: usize = 16;

#[derive(Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = [f32])]
pub struct CameraPose(pub [f32; 6]);

pub struct PoseState {
    camera_pose: RwLock<CameraPose>,
    pose_updates: broadcast::Sender<CameraPose>,
}

impl PoseState {
    pub fn new() -> Self {
        let (pose_updates, _) = broadcast::channel(UPDATE_BUFFER_CAPACITY);
        Self {
            camera_pose: RwLock::new(CameraPose([0.0; 6])),
            pose_updates,
        }
    }

    pub fn get(&self) -> CameraPose {
        *self.camera_pose.read().expect("camera pose lock poisoned")
    }

    pub fn update(&self, camera_pose: CameraPose) {
        *self.camera_pose.write().expect("camera pose lock poisoned") = camera_pose;
        self.publish(camera_pose);
    }


    pub fn subscribe(&self) -> broadcast::Receiver<CameraPose> {
        self.pose_updates.subscribe()
    }

    fn publish(&self, camera_pose: CameraPose) {
        let _ = self.pose_updates.send(camera_pose);
    }
}

pub type SharedPoseState = Arc<PoseState>;
