mod api;
mod osc;
mod pose;

use std::{net::SocketAddr, sync::Arc};

use tokio::net::TcpListener;
use vrchat_osc::{Error, VRChatOSC};

use crate::pose::PoseState;

#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("vrchat_osc", log::LevelFilter::Warn)
        .init();

    let pose_state = Arc::new(PoseState::new());
    let app = api::router(Arc::clone(&pose_state));
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 3000))).await?;
    let port = listener.local_addr()?.port();

    let vrchat_osc = VRChatOSC::new(None).await?;
    osc::register_usercamera_service(&vrchat_osc, pose_state).await?;

    log::info!("Pose API: http://localhost:{port}/usercamera/pose");
    log::info!("Pose WebSocket: ws://localhost:{port}/usercamera/pose/ws");
    log::info!("API documentation: http://localhost:{port}/swagger-ui/");
    log::info!("Press Ctrl+C to exit.");

    tokio::select! {
        result = axum::serve(listener, app) => result?,
        result = tokio::signal::ctrl_c() => result?,
    }

    vrchat_osc.shutdown().await?;
    Ok(())
}