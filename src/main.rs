mod api;
mod capture;
mod osc;
mod parameters;
mod pose;

use std::{net::SocketAddr, sync::Arc};

use axum::{ServiceExt, extract::Request};
use tower_http::normalize_path::NormalizePath;

use tokio::net::TcpListener;
use vrchat_osc::VRChatOSC;

use crate::pose::PoseState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("vrchat_osc", log::LevelFilter::Warn)
        .init();

    let pose_state = Arc::new(PoseState::new());
    let capture_state = Arc::new(capture::CaptureState::new()?);
    let vrchat_osc = VRChatOSC::new(None).await?;
    let camera_stream_state = Arc::new(osc::CameraStreamState::default());
    let app = NormalizePath::trim_trailing_slash(api::router(
        Arc::clone(&pose_state),
        Arc::clone(&vrchat_osc),
        Arc::clone(&capture_state),
        Arc::clone(&camera_stream_state),
    ));
    let app = <NormalizePath<axum::Router> as ServiceExt<Request>>::into_make_service(app);
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 3000))).await?;
    let port = listener.local_addr()?.port();

    osc::register_usercamera_service(&vrchat_osc, pose_state, Arc::clone(&camera_stream_state))
        .await?;
    if let Err(error) = osc::initialize_camera_stream_state(&vrchat_osc, &camera_stream_state).await
    {
        log::warn!("Unable to query initial VRChat camera state: {error}");
    }

    log::info!("Pose API: http://localhost:{port}/usercamera/pose");
    log::info!("OSC API: http://localhost:{port}/osc");
    log::info!("Pose WebSocket: ws://localhost:{port}/usercamera/pose/ws");
    log::info!("Capture API: http://localhost:{port}/usercamera/capture");
    log::info!("API documentation: http://localhost:{port}/swagger-ui/");
    log::info!("Press Ctrl+C to exit.");

    tokio::select! {
        result = axum::serve(listener, app) => result?,
        result = tokio::signal::ctrl_c() => result?,
    }

    vrchat_osc.shutdown().await?;
    Ok(())
}
