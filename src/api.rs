use std::sync::Arc;

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    capture::CaptureState,
    osc,
    pose::{CameraPose, SharedPoseState},
};

#[derive(OpenApi)]
#[openapi(
    paths(
        get_camera_pose,
        set_camera_pose,
        capture_camera_pose,
        camera_pose_websocket
    ),
    components(schemas(CameraPose)),
    info(title = "User Camera Pose API", version = "0.1.0")
)]
struct ApiDoc;

#[derive(Clone)]
struct ApiState {
    pose_state: SharedPoseState,
    vrchat_osc: Arc<vrchat_osc::VRChatOSC>,
    capture_state: Arc<CaptureState>,
    capture_request_lock: Arc<tokio::sync::Mutex<()>>,
}

pub fn router(
    pose_state: SharedPoseState,
    vrchat_osc: Arc<vrchat_osc::VRChatOSC>,
    capture_state: Arc<CaptureState>,
) -> Router {
    Router::new()
        .route(
            "/usercamera/pose",
            get(get_camera_pose).post(set_camera_pose),
        )
        .route("/usercamera/pose/ws", get(camera_pose_websocket))
        .route("/usercamera/capture", post(capture_camera_pose))
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(ApiState {
            pose_state,
            vrchat_osc,
            capture_state,
            capture_request_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
}

#[utoipa::path(
    get,
    path = "/usercamera/pose",
    responses((status = 200, description = "Current camera pose.", body = CameraPose))
)]
async fn get_camera_pose(State(state): State<ApiState>) -> Json<CameraPose> {
    Json(state.pose_state.get())
}

#[utoipa::path(
    post,
    path = "/usercamera/pose",
    request_body = CameraPose,
    responses(
        (status = 204, description = "Camera pose sent to VRChat and updated."),
        (status = 422, description = "Request body is not a six-element pose array."),
        (status = 502, description = "Unable to send the camera pose to VRChat.")
    )
)]
async fn set_camera_pose(
    State(state): State<ApiState>,
    Json(camera_pose): Json<CameraPose>,
) -> StatusCode {
    if let Err(error) = osc::send_camera_pose(&state.vrchat_osc, camera_pose).await {
        log::error!("Failed to send camera pose to VRChat: {error}");
        return StatusCode::BAD_GATEWAY;
    }

    state.pose_state.update(camera_pose);
    StatusCode::NO_CONTENT
}

#[utoipa::path(
    post,
    path = "/usercamera/capture",
    request_body = CameraPose,
    responses(
        (status = 200, description = "PNG frame from the camera at the requested pose.", content_type = "image/png"),
        (status = 422, description = "Request body is not a six-element pose array."),
        (status = 502, description = "Unable to configure or move the VRChat camera."),
        (status = 503, description = "Spout capture is unavailable or did not produce a frame.")
    )
)]
async fn capture_camera_pose(
    State(state): State<ApiState>,
    Json(camera_pose): Json<CameraPose>,
) -> Result<Response, (StatusCode, String)> {
    // A Spout frame has no pose metadata. Serializing the full request prevents a later
    // request from moving the camera while this request waits for its post-move frame.
    let _capture_request_guard = state.capture_request_lock.lock().await;

    if let Err(error) = osc::configure_camera_stream(&state.vrchat_osc).await {
        log::error!("Failed to configure VRChat camera streaming: {error}");
        return Err((
            StatusCode::BAD_GATEWAY,
            "unable to configure VRChat camera streaming".to_owned(),
        ));
    }
    if let Err(error) = osc::send_camera_pose(&state.vrchat_osc, camera_pose).await {
        log::error!("Failed to send capture camera pose to VRChat: {error}");
        return Err((
            StatusCode::BAD_GATEWAY,
            "unable to move the VRChat camera".to_owned(),
        ));
    }
    state.pose_state.update(camera_pose);

    // Queue the request before returning. The worker receives the first frame it can
    // read after this post-pose settling interval, rather than tying an HTTP worker to
    // a blocking Direct3D readback.
    let png = state
        .capture_state
        .capture_png_after(std::time::Duration::from_millis(250))
        .await
        .map_err(|error| {
            log::error!("Failed to capture Spout frame: {error}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Spout did not produce a camera frame".to_owned(),
            )
        })?;
    Ok((
        [(header::CONTENT_TYPE, HeaderValue::from_static("image/png"))],
        png,
    )
        .into_response())
}

#[utoipa::path(
    get,
    path = "/usercamera/pose/ws",
    responses((status = 101, description = "Streams each subsequent camera pose as a JSON array."))
)]
async fn camera_pose_websocket(
    websocket: WebSocketUpgrade,
    State(state): State<ApiState>,
) -> Response {
    websocket.on_upgrade(move |socket| stream_camera_poses(socket, state.pose_state.subscribe()))
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
