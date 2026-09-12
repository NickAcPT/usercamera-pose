use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::{Json, Response},
    routing::get,
    Router,
};
use tokio::{
    net::TcpListener,
    sync::{broadcast, RwLock},
};
use vrchat_osc::{
    models::OscRootNode,
    rosc::{OscPacket, OscType},
    Error, VRChatOSC,
};

type CameraPose = [f32; 6];

struct AppState {
    camera_pose: RwLock<CameraPose>,
    pose_updates: broadcast::Sender<CameraPose>,
}

type SharedState = Arc<AppState>;

async fn get_camera_pose(State(state): State<SharedState>) -> Json<CameraPose> {
    Json(*state.camera_pose.read().await)
}

async fn set_camera_pose(
    State(state): State<SharedState>,
    Json(camera_pose): Json<CameraPose>,
) -> StatusCode {
    *state.camera_pose.write().await = camera_pose;
    let _ = state.pose_updates.send(camera_pose);
    StatusCode::NO_CONTENT
}

async fn camera_pose_websocket(
    websocket: WebSocketUpgrade,
    State(state): State<SharedState>,
) -> Response {
    websocket.on_upgrade(move |socket| stream_camera_poses(socket, state.pose_updates.subscribe()))
}

async fn stream_camera_poses(
    mut socket: WebSocket,
    mut pose_updates: broadcast::Receiver<CameraPose>,
) {
    loop {
        match pose_updates.recv().await {
            Ok(camera_pose) => match serde_json::to_string(&camera_pose) {
                Ok(message) => {
                    if let Err(error) = socket.send(Message::Text(message.into())).await {
                        log::debug!("Camera pose websocket closed with an error: {error}");
                        break;
                    }
                }
                Err(error) => log::error!("Failed to serialize camera pose for websocket: {error}"),
            },
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                log::warn!("Camera pose websocket skipped {skipped} updates");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}


#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let (pose_updates, _) = broadcast::channel(16);
    let pose = Arc::new(AppState {
        camera_pose: RwLock::new([0.0; 6]),
        pose_updates,
    });
    let app = Router::new()
        .route(
            "/usercamera/pose",
            get(get_camera_pose).post(set_camera_pose),
        )
        .route("/usercamera/pose/ws", get(camera_pose_websocket))
        .with_state(Arc::clone(&pose));
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 3000))).await?;
    log::info!("HTTP API listening on http://{}/usercamera/pose", listener.local_addr()?);

    // Initialize VRChatOSC instance.
    // When None is passed, automatically selects a non-loopback IPv4 interface.
    // Pass Some(IpAddr) to advertise on a specific network interface
    let vrchat_osc = VRChatOSC::new(None).await?;

    let root_node = OscRootNode::new().with_usercamera();
    let osc_pose = Arc::clone(&pose);
    vrchat_osc
        .register("NickUserCameraPose", root_node, move |packet| {
            if let OscPacket::Message(msg) = packet && msg.addr == "/usercamera/Pose" {
                let data: CameraPose = match &msg.args[..] {
                    [OscType::Float(a), OscType::Float(b), OscType::Float(c), OscType::Float(d), OscType::Float(e), OscType::Float(f)] => [*a, *b, *c, *d, *e, *f],
                    _ => {
                        log::error!("Unexpected number of arguments in OSC message: {:?}", msg.args);
                        return;
                    }
                };
                *osc_pose.camera_pose.blocking_write() = data;
                let _ = osc_pose.pose_updates.send(data);
                log::info!("Received OSC message: {:?}", data);
            }
        })
        .await?;
    log::info!("Service registered.");

    log::info!("Press Ctrl+C to exit.");
    tokio::select! {
        result = axum::serve(listener, app) => result?,
        result = tokio::signal::ctrl_c() => result?,
    }

    vrchat_osc.shutdown().await?;
    Ok(())
}