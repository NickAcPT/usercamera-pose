use std::{
    collections::VecDeque,
    fmt::{Display, Formatter},
    io::Cursor,
    time::{Duration, Instant},
};

use spout_rs::{Directory, SpoutDxReceiver};
use tokio::sync::{mpsc, oneshot};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
            },
            Dxgi::{
                Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC},
                CreateDXGIFactory1, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, IDXGIAdapter,
                IDXGIAdapter1, IDXGIFactory6,
            },
        },
    },
    core::Interface,
};

const VRC_SPOUT_SENDER: &str = "VRCSender1";
const REQUEST_QUEUE_CAPACITY: usize = 16;
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(1);
const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;
const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28;

type FrameReply = oneshot::Sender<Result<Vec<u8>, CaptureError>>;

pub struct CaptureState {
    requests: mpsc::Sender<CaptureRequest>,
}

struct CaptureRequest {
    not_before: Instant,
    reply: FrameReply,
}

impl CaptureState {
    pub fn new() -> Result<Self, CaptureInitError> {
        let (requests, receiver) = mpsc::channel(REQUEST_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("spout-capture".to_owned())
            .spawn(move || capture_worker(receiver))
            .map_err(CaptureInitError::Worker)?;
        Ok(Self { requests })
    }

    pub async fn capture_png_after(&self, delay: Duration) -> Result<Vec<u8>, CaptureError> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(CaptureRequest {
                not_before: Instant::now() + delay,
                reply,
            })
            .await
            .map_err(|_| CaptureError::WorkerStopped)?;

        tokio::time::timeout(FRAME_TIMEOUT + delay, response)
            .await
            .map_err(|_| CaptureError::Timeout)?
            .map_err(|_| CaptureError::WorkerStopped)?
    }
}

#[derive(Debug)]
pub enum CaptureInitError {
    Worker(std::io::Error),
}

impl Display for CaptureInitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Worker(error) => write!(
                formatter,
                "unable to start the Spout capture worker: {error}"
            ),
        }
    }
}

impl std::error::Error for CaptureInitError {}

#[derive(Debug)]
pub enum CaptureError {
    Timeout,
    WorkerStopped,
    D3d(String),
    UnsupportedFormat(u32),
    Png(png::EncodingError),
}

impl Display for CaptureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(
                formatter,
                "no Spout frame arrived before the capture timeout"
            ),
            Self::WorkerStopped => write!(formatter, "the Spout capture worker stopped"),
            Self::D3d(error) => write!(formatter, "Direct3D Spout receive failed: {error}"),
            Self::UnsupportedFormat(format) => {
                write!(formatter, "unsupported Spout DXGI format {format}")
            }
            Self::Png(error) => write!(formatter, "PNG encoding failed: {error}"),
        }
    }
}

impl std::error::Error for CaptureError {}

fn capture_worker(mut requests: mpsc::Receiver<CaptureRequest>) {
    let mut pending = VecDeque::new();
    let mut reader = None;
    let mut last_reader_error = None;

    loop {
        while let Ok(request) = requests.try_recv() {
            pending.push_back(request);
        }
        if pending.is_empty() {
            match requests.blocking_recv() {
                Some(request) => pending.push_back(request),
                None => return,
            }
        }

        let Some(request) = pending.front() else {
            continue;
        };
        if Instant::now() < request.not_before {
            std::thread::sleep(POLL_INTERVAL);
            continue;
        }

        if reader.is_none() {
            match create_reader() {
                Ok(new_reader) => {
                    last_reader_error = None;
                    reader = Some(new_reader);
                }
                Err(error) => {
                    let error_message = error.to_string();
                    if last_reader_error.as_deref() != Some(error_message.as_str()) {
                        log::warn!("Spout capture reader initialization failed: {error_message}");
                    }
                    last_reader_error = Some(error_message);
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }
            }
        }

        let Some(dx_reader) = reader.as_mut() else {
            std::thread::sleep(POLL_INTERVAL);
            continue;
        };
        match dx_reader.read_frame() {
            Ok(Some(())) => {
                let result = encode_png(
                    dx_reader.width,
                    dx_reader.height,
                    dx_reader.dxgi_format,
                    &dx_reader.scratch,
                );
                let request = pending.pop_front().expect("queue front exists");
                let _ = request.reply.send(result);
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                log::warn!("Spout frame read failed; reconnecting: {error}");
                reader = None;
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

fn create_reader() -> Result<DxReader, CaptureError> {
    let info = Directory::sender_texture_info(VRC_SPOUT_SENDER)
        .ok_or_else(|| CaptureError::D3d("VRChat's Spout sender is unavailable".to_owned()))?;
    if info.width == 0 || info.height == 0 {
        return Err(CaptureError::D3d(
            "VRChat's Spout sender has no frame dimensions".to_owned(),
        ));
    }
    DxReader::new(VRC_SPOUT_SENDER, info.width, info.height, info.format)
}

struct DxReader {
    receiver: SpoutDxReceiver,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    target: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    dxgi_format: u32,
    scratch: Vec<u8>,
}

impl DxReader {
    fn new(sender: &str, width: u32, height: u32, dxgi_format: u32) -> Result<Self, CaptureError> {
        let mut device = None;
        let mut context = None;
        let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory1() }
            .map_err(|error| CaptureError::D3d(error.to_string()))?;
        let adapter1: IDXGIAdapter1 =
            unsafe { factory.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }
                .map_err(|error| CaptureError::D3d(error.to_string()))?;
        let adapter: IDXGIAdapter = adapter1
            .cast()
            .map_err(|error| CaptureError::D3d(error.to_string()))?;
        unsafe {
            D3D11CreateDevice(
                Some(&adapter),
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|error| CaptureError::D3d(error.to_string()))?;
        }
        let device = device
            .ok_or_else(|| CaptureError::D3d("device creation returned no device".to_owned()))?;
        let context = context
            .ok_or_else(|| CaptureError::D3d("device creation returned no context".to_owned()))?;
        let (target, staging) = Self::create_textures(&device, width, height, dxgi_format)?;
        let mut receiver = SpoutDxReceiver::new(Some(sender));
        if !receiver.open_directx11(device.as_raw() as usize) {
            return Err(CaptureError::D3d(
                "Spout could not open the D3D11 receiver".to_owned(),
            ));
        }

        Ok(Self {
            receiver,
            device,
            context,
            target,
            staging,
            width,
            height,
            dxgi_format,
            scratch: Vec::new(),
        })
    }

    fn create_textures(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        dxgi_format: u32,
    ) -> Result<(ID3D11Texture2D, ID3D11Texture2D), CaptureError> {
        let base = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT(dxgi_format as i32),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut target = None;
        unsafe {
            device
                .CreateTexture2D(&base, None, Some(&mut target))
                .map_err(|error| CaptureError::D3d(error.to_string()))?;
        }
        let staging_descriptor = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..base
        };
        let mut staging = None;
        unsafe {
            device
                .CreateTexture2D(&staging_descriptor, None, Some(&mut staging))
                .map_err(|error| CaptureError::D3d(error.to_string()))?;
        }
        Ok((
            target.ok_or_else(|| {
                CaptureError::D3d("texture creation returned no target".to_owned())
            })?,
            staging.ok_or_else(|| {
                CaptureError::D3d("texture creation returned no staging texture".to_owned())
            })?,
        ))
    }

    fn read_frame(&mut self) -> Result<Option<()>, CaptureError> {
        if !self.receiver.receive_texture(self.target.as_raw() as usize) {
            return Ok(None);
        }
        if self.receiver.is_updated() {
            let (width, height) = self.receiver.sender_size();
            let format = self.receiver.sender_format();
            if width == 0 || height == 0 {
                return Ok(None);
            }
            let (target, staging) = Self::create_textures(&self.device, width, height, format)?;
            self.target = target;
            self.staging = staging;
            self.width = width;
            self.height = height;
            self.dxgi_format = format;
            if !self.receiver.receive_texture(self.target.as_raw() as usize) {
                return Ok(None);
            }
        }
        if !self.receiver.is_frame_new() {
            return Ok(None);
        }

        unsafe {
            self.context.CopyResource(&self.staging, &self.target);
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|error| CaptureError::D3d(error.to_string()))?;
        }
        let length = mapped.RowPitch as usize * self.height as usize;
        self.scratch.clear();
        unsafe {
            self.scratch.extend_from_slice(std::slice::from_raw_parts(
                mapped.pData.cast::<u8>(),
                length,
            ));
            self.context.Unmap(&self.staging, 0);
        }
        Ok(Some(()))
    }
}

fn encode_png(
    width: u32,
    height: u32,
    format: u32,
    pixels: &[u8],
) -> Result<Vec<u8>, CaptureError> {
    let packed_row_len = width as usize * 4;
    let packed_len = packed_row_len * height as usize;
    let row_pitch = pixels.len() / height as usize;
    if row_pitch < packed_row_len || pixels.len() != row_pitch * height as usize {
        return Err(CaptureError::D3d(
            "Spout staging texture has an invalid row pitch".to_owned(),
        ));
    }

    let mut rgba = Vec::with_capacity(packed_len);
    for row in pixels.chunks_exact(row_pitch) {
        rgba.extend_from_slice(&row[..packed_row_len]);
    }
    match format {
        DXGI_FORMAT_R8G8B8A8_UNORM => {}
        DXGI_FORMAT_B8G8R8A8_UNORM => {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        _ => return Err(CaptureError::UnsupportedFormat(format)),
    }

    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(Cursor::new(&mut output), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(CaptureError::Png)?;
    writer.write_image_data(&rgba).map_err(CaptureError::Png)?;
    drop(writer);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{DXGI_FORMAT_B8G8R8A8_UNORM, encode_png};

    #[test]
    fn encodes_bgra_spout_pixels_as_rgba_png() {
        let png = encode_png(1, 1, DXGI_FORMAT_B8G8R8A8_UNORM, &[3, 2, 1, 255]).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();

        assert_eq!((info.width, info.height), (1, 1));
        assert_eq!(&decoded[..4], [1, 2, 3, 255]);
    }
}
