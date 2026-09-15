use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    capture::CaptureState,
    osc::{self, CameraStreamState},
    parameters::{self, SendError},
    pose::{CameraPose, SharedPoseState},
};

const CAPTURE_SETTLING_DELAY: Duration = Duration::from_millis(5);

#[derive(OpenApi)]
#[openapi(
    paths(
        get_camera_pose,
        set_camera_pose,
        get_parameters,
        get_parameter_value,
        set_parameter,
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
    camera_stream_state: Arc<CameraStreamState>,
    capture_request_lock: Arc<tokio::sync::Mutex<()>>,
}

pub fn router(
    pose_state: SharedPoseState,
    vrchat_osc: Arc<vrchat_osc::VRChatOSC>,
    capture_state: Arc<CaptureState>,
    camera_stream_state: Arc<CameraStreamState>,
) -> Router {
    Router::new()
        .route(
            "/usercamera/pose",
            get(get_camera_pose).post(set_camera_pose),
        )
        .route("/osc", get(get_parameters))
        .route(
            "/osc/{*parameter}",
            get(get_parameter_value).post(set_parameter),
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
            camera_stream_state,
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
    if let Err(error) = send_and_store_camera_pose(&state, camera_pose).await {
        log::error!("Failed to send camera pose to VRChat: {error}");
        return StatusCode::BAD_GATEWAY;
    }

    StatusCode::NO_CONTENT
}

async fn send_and_store_camera_pose(
    state: &ApiState,
    camera_pose: CameraPose,
) -> Result<(), vrchat_osc::Error> {
    osc::send_camera_pose(&state.vrchat_osc, camera_pose).await?;
    state.pose_state.update(camera_pose);
    Ok(())
}

#[utoipa::path(
    get,
    path = "/osc",
    responses(
        (status = 200, description = "VRChat's OSCQuery parameter tree, including each parameter's TYPE."),
        (status = 503, description = "VRChat's OSCQuery service is unavailable."),
        (status = 502, description = "Unable to query VRChat's OSCQuery service.")
    )
)]
async fn get_parameters(
    State(state): State<ApiState>,
) -> Result<Json<vrchat_osc::models::OscNode>, (StatusCode, String)> {
    let mut parameter_tree = parameters::get_all(&state.vrchat_osc)
        .await
        .map_err(|error| {
            log::error!("Failed to query VRChat parameters: {error}");
            (
                StatusCode::BAD_GATEWAY,
                "unable to query VRChat parameters".to_owned(),
            )
        })?
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "VRChat's OSCQuery service is unavailable".to_owned(),
        ))?;

    parameters::set_value(
        &mut parameter_tree,
        osc::USER_CAMERA_POSE_ADDRESS,
        state
            .pose_state
            .get()
            .0
            .into_iter()
            .map(|value| vrchat_osc::models::OscValue::Float(f64::from(value)))
            .collect(),
    );
    Ok(Json(parameter_tree))
}

#[utoipa::path(
    get,
    path = "/osc/{parameter}",
    params(("parameter" = String, Path, description = "OSC parameter path without its leading slash.")),
    responses(
        (status = 200, description = "Current OSC parameter value."),
        (status = 404, description = "Parameter was not found or has no current value."),
        (status = 502, description = "Unable to query VRChat's OSCQuery service.")
    )
)]
async fn get_parameter_value(
    State(state): State<ApiState>,
    Path(parameter): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let parameter = format!("/{parameter}");
    if parameter == osc::USER_CAMERA_POSE_ADDRESS {
        return serde_json::to_value(state.pose_state.get())
            .map(Json)
            .map_err(|error| {
                log::error!("Failed to serialize the user camera pose: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "unable to serialize the user camera pose".to_owned(),
                )
            });
    }

    let node = parameters::get(&state.vrchat_osc, &parameter)
        .await
        .map_err(|error| {
            log::error!("Failed to query VRChat parameter {parameter}: {error}");
            (
                StatusCode::BAD_GATEWAY,
                "unable to query VRChat parameter".to_owned(),
            )
        })?
        .ok_or((StatusCode::NOT_FOUND, "parameter was not found".to_owned()))?;
    parameters::value(node)
        .map_err(|error| {
            log::error!("Failed to serialize VRChat parameter {parameter}: {error}");
            (
                StatusCode::BAD_GATEWAY,
                "unable to serialize VRChat parameter".to_owned(),
            )
        })?
        .map(Json)
        .ok_or((
            StatusCode::NOT_FOUND,
            "parameter has no current value".to_owned(),
        ))
}

#[utoipa::path(
    post,
    path = "/osc/{parameter}",
    params(("parameter" = String, Path, description = "OSC parameter path without its leading slash.")),
    request_body = Value,
    responses(
        (status = 204, description = "Parameter sent to VRChat."),
        (status = 403, description = "The user camera pose is set only through /usercamera/pose."),
        (status = 404, description = "Parameter was not found in VRChat's OSCQuery tree."),
        (status = 422, description = "Request body does not match the parameter TYPE."),
        (status = 502, description = "Unable to query or send the parameter to VRChat.")
    )
)]
async fn set_parameter(
    State(state): State<ApiState>,
    Path(parameter): Path<String>,
    Json(value): Json<Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    let parameter = format!("/{parameter}");
    if parameter == osc::USER_CAMERA_POSE_ADDRESS {
        return Err((
            StatusCode::FORBIDDEN,
            "set the user camera pose through /usercamera/pose".to_owned(),
        ));
    }

    parameters::send(&state.vrchat_osc, &parameter, value)
        .await
        .map_err(|error| {
            let status = match &error {
                SendError::NotFound => StatusCode::NOT_FOUND,
                SendError::UnsupportedType | SendError::InvalidValue(_) => {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
                SendError::Query(_) | SendError::Send(_) => StatusCode::BAD_GATEWAY,
            };
            log::error!("Failed to set VRChat parameter {parameter}: {error}");
            (status, error.to_string())
        })?;
    Ok(StatusCode::NO_CONTENT)
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

    if let Err(error) =
        osc::configure_camera_stream(&state.vrchat_osc, &state.camera_stream_state).await
    {
        log::error!("Failed to configure VRChat camera streaming: {error}");
        return Err((
            StatusCode::BAD_GATEWAY,
            "unable to configure VRChat camera streaming".to_owned(),
        ));
    }

    if let Err(error) = osc::send_camera_pose(&state.vrchat_osc, camera_pose).await {
        log::error!("Failed to send capture camera pose: {error}");
        return Err((
            StatusCode::BAD_GATEWAY,
            "unable to move the VRChat camera".to_owned(),
        ));
    }
    state.pose_state.update(camera_pose);

    // Spout carries no pose metadata. Give VRChat 5 ms to apply the OSC command, then
    // capture the next Spout frame it publishes.
    let png = state
        .capture_state
        .capture_png_after(CAPTURE_SETTLING_DELAY)
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
