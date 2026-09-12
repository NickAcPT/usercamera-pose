use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Json, Response},
    routing::get,
    Router,
};
use tokio::{net::TcpListener, sync::RwLock};
use vrchat_osc::{
    models::OscRootNode,
    rosc::{OscPacket, OscType},
    Error, VRChatOSC,
};

type CameraPose = [f32; 6];
type AppState = Arc<RwLock<CameraPose>>;

async fn get_camera_pose(State(pose): State<AppState>) -> Json<CameraPose> {
    Json(*pose.read().await)
}

async fn camera_pose_websocket(
    websocket: WebSocketUpgrade,
    State(pose): State<AppState>,
) -> Response {
    websocket.on_upgrade(move |socket| receive_camera_poses(socket, pose))
}

async fn receive_camera_poses(mut socket: WebSocket, pose: AppState) {
    while let Some(result) = socket.recv().await {
        match result {
            Ok(Message::Text(message)) => match serde_json::from_str::<CameraPose>(&message) {
                Ok(camera_pose) => *pose.write().await = camera_pose,
                Err(error) => log::warn!("Ignoring invalid camera pose websocket message: {error}"),
            },
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                log::debug!("Camera pose websocket closed with an error: {error}");
                break;
            }
        }
    }
}


#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let pose = Arc::new(RwLock::new([0.0; 6]));
    let app = Router::new()
        .route("/usercamera/pose", get(get_camera_pose))
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
                *osc_pose.blocking_write() = data;
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