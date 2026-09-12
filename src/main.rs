use std::time::Duration;

use tokio;
use vrchat_osc::{
    models::OscRootNode,
    rosc::{OscPacket, OscType},
    Error, VRChatOSC,
};


#[tokio::main]
async fn main() -> Result<(), Error> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .init();

    // Initialize VRChatOSC instance.
    // When None is passed, automatically selects a non-loopback IPv4 interface.
    // Pass Some(IpAddr) to advertise on a specific network interface
    let vrchat_osc = VRChatOSC::new(None).await?;

    let root_node = OscRootNode::new().with_usercamera();
    vrchat_osc
        .register("NickUserCameraPose", root_node, |packet| {
            if let OscPacket::Message(msg) = packet && msg.addr == "/usercamera/Pose" {
                let data: [f32; 6] = match &msg.args[..] {
                    [OscType::Float(a), OscType::Float(b), OscType::Float(c), OscType::Float(d), OscType::Float(e), OscType::Float(f)] => [*a, *b, *c, *d, *e, *f],
                    _ => {
                        log::error!("Unexpected number of arguments in OSC message: {:?}", msg.args);
                        return;
                    }
                };
                log::info!("Received OSC message: {:?}", data);
            }
        })
        .await?;
    log::info!("Service registered.");

    // Wait for the service to be registered
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Keep the program running to handle incoming messages
    log::info!("Press Ctrl+C to exit.");
    tokio::signal::ctrl_c().await?;

    // Shutdown the VRChatOSC instance
    vrchat_osc.shutdown().await?;
    Ok(())
}