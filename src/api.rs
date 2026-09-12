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
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::pose::{CameraPose, SharedPoseState};

#[derive(OpenApi)]
#[openapi(
    paths(get_camera_pose, set_camera_pose, camera_pose_websocket),
    components(schemas(CameraPose)),
    info(title = "User Camera Pose API", version = "0.1.0")
)]
struct ApiDoc;

pub fn router(pose_state: SharedPoseState) -> Router {
    Router::new()
        .route(
            "/usercamera/pose",
            get(get_camera_pose).post(set_camera_pose),
        )
        .route("/usercamera/pose/ws", get(camera_pose_websocket))
        .merge(
            SwaggerUi::new("/swagger-ui")
                .url("/api-docs/openapi.json", ApiDoc::openapi()),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(pose_state)
}

#[utoipa::path(
    get,
    path = "/usercamera/pose",
    responses((status = 200, description = "Current camera pose.", body = CameraPose))
)]
async fn get_camera_pose(State(pose_state): State<SharedPoseState>) -> Json<CameraPose> {
    Json(pose_state.get().await)
}

#[utoipa::path(
    post,
    path = "/usercamera/pose",
    request_body = CameraPose,
    responses(
        (status = 204, description = "Camera pose updated."),
        (status = 422, description = "Request body is not a six-element pose array.")
    )
)]
async fn set_camera_pose(
    State(pose_state): State<SharedPoseState>,
    Json(camera_pose): Json<CameraPose>,
) -> StatusCode {
    pose_state.update(camera_pose).await;
    StatusCode::NO_CONTENT
}

#[utoipa::path(
    get,
    path = "/usercamera/pose/ws",
    responses((status = 101, description = "Streams each subsequent camera pose as a JSON array."))
)]
async fn camera_pose_websocket(
    websocket: WebSocketUpgrade,
    State(pose_state): State<SharedPoseState>,
) -> Response {
    websocket.on_upgrade(move |socket| stream_camera_poses(socket, pose_state.subscribe()))
}

async fn stream_camera_poses(
    mut socket: WebSocket,
    mut pose_updates: tokio::sync::broadcast::Receiver<CameraPose>,
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
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                log::warn!("Camera pose websocket skipped {skipped} updates");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
