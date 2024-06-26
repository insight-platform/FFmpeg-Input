use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};
use std::time::SystemTime;

use crossbeam::channel::{Receiver, Sender};
use ffmpeg::util::frame::video::Video;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::Id;
use ffmpeg_next::format::{input_with_dictionary, Pixel};
use ffmpeg_next::log::Level;
use ffmpeg_next::software::converter;
use log::{debug, error, info, warn};
use parking_lot::Mutex;
use pyo3::exceptions::PyBrokenPipeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

const DECODING_FORMAT: Pixel = Pixel::RGB24;
const DECODED_PIX_BYTES: u32 = 3;

fn is_stream_key_framed(id: Id) -> Result<bool, String> {
    let key_frames = match id {
        Id::H264
        | Id::H265
        | Id::HEVC
        | Id::VP9
        | Id::VP8
        | Id::AV1
        | Id::MPEG1VIDEO
        | Id::MPEG2VIDEO
        | Id::MPEG4
        | Id::MSMPEG4V1
        | Id::MSMPEG4V2
        | Id::MSMPEG4V3
        | Id::THEORA
        | Id::FLV1 => Some(true),
        Id::MJPEG | Id::TIFF | Id::PNG | Id::JPEG2000 | Id::RAWVIDEO => Some(false),
        _ => None,
    };

    match key_frames {
        Some(v) => Ok(v),
        None => Err(format!("{:?}", id)),
    }
}

#[derive(Debug, Clone)]
#[pyclass]
pub struct VideoFrameEnvelope {
    #[pyo3(get)]
    pub codec: String,
    #[pyo3(get)]
    pub frame_width: i64,
    #[pyo3(get)]
    pub frame_height: i64,
    #[pyo3(get)]
    pub key_frame: bool,
    #[pyo3(get)]
    pub time_base: (i64, i64),
    #[pyo3(get)]
    pub pts: Option<i64>,
    #[pyo3(get)]
    pub dts: Option<i64>,
    #[pyo3(get)]
    pub corrupted: bool,
    #[pyo3(get)]
    pub fps: String,
    #[pyo3(get)]
    pub avg_fps: String,
    #[pyo3(get)]
    pub pixel_format: String,
    #[pyo3(get)]
    pub frame_received_ts: i64,
    #[pyo3(get)]
    pub frame_processed_ts: i64,
    #[pyo3(get)]
    pub queue_len: i64,
    #[pyo3(get)]
    pub queue_full_skipped_count: i64,
    #[pyo3(get)]
    pub payload: Vec<u8>,
}

#[pyclass]
#[derive(Debug, Clone)]
pub enum FFmpegLogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Trace,
    Quiet,
    Fatal,
    Panic,
}

#[pymethods]
impl VideoFrameEnvelope {
    #[classattr]
    const __hash__: Option<Py<PyAny>> = None;

    fn __repr__(&self) -> String {
        format!("{:?}", self)
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }

    fn payload_as_bytes(&self, py: Python) -> PyResult<PyObject> {
        let res = PyBytes::new_with(py, self.payload.len(), |b: &mut [u8]| {
            b.copy_from_slice(&self.payload);
            Ok(())
        })?;
        Ok(res.into())
    }
}

#[pyclass]
pub struct FFMpegSource {
    video_source: Receiver<VideoFrameEnvelope>,
    thread: Option<JoinHandle<()>>,
    exit_signal: Arc<Mutex<bool>>,
    log_level: Arc<Mutex<Option<Level>>>,
}

impl Drop for FFMpegSource {
    fn drop(&mut self) {
        {
            let mut exit_signal = self.exit_signal.lock();
            *exit_signal = true;
        }
        let t = self.thread.take();
        t.unwrap().join().expect("Thread must be finished normally");
        debug!("Worker thread is terminated");
    }
}

#[allow(clippy::too_many_arguments)]
fn handle(
    uri: String,
    params: HashMap<String, String>,
    tx: Sender<VideoFrameEnvelope>,
    signal: Arc<Mutex<bool>>,
    decode: bool,
    autoconvert_raw_formats_to_rgb24: bool,
    block_if_queue_full: bool,
    log_level: Arc<Mutex<Option<Level>>>,
) {
    let mut queue_full_skipped_count = 0;
    ffmpeg::init().expect("FFmpeg initialization must be successful");
    let ll = log_level.lock().take();

    if let Some(l) = ll {
        info!("Setting log level to {:?}", l);
        ffmpeg::log::set_level(l);
    }

    let mut opts = ffmpeg::Dictionary::new();
    for kv in &params {
        opts.set(kv.0.as_str(), kv.1.as_str());
    }
    let p = Path::new(uri.as_str());

    let mut ictx = input_with_dictionary(&p, opts).expect("Input stream must be initialized");

    let video_input = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap_or_else(|| panic!("Cannot discover the best suitable video stream to work with."));

    let video_stream_index = video_input.index();

    let mut video_decoder =
        ffmpeg::codec::context::Context::from_parameters(video_input.parameters())
            .and_then(|c| c.decoder().video())
            .expect("Video decoder must be available");

    let mut converter = converter(
        (video_decoder.width(), video_decoder.height()),
        video_decoder.format(),
        DECODING_FORMAT,
    )
    .expect("Video scaler must be initialized");

    // let mut video_scaler = Context::get(
    //     video_decoder.format(),
    //     video_decoder.width(),
    //     video_decoder.height(),
    //     Pixel::rgb24,
    //     video_decoder.width(),
    //     video_decoder.height(),
    //     Flags::FAST_BILINEAR,
    // )
    // .expect("Video scaler must be initialized");

    let audio_stream_index_opt = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .map(|s| s.index());

    let audio_opt = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Audio)
        .and_then(|s| ffmpeg::codec::context::Context::from_parameters(s.parameters()).ok())
        .and_then(|c| c.decoder().audio().ok());

    let mut skip_until_first_key_frame = true;
    for (stream, packet) in ictx.packets() {
        if *signal.lock() {
            break;
        }
        let ll = log_level.lock().take();

        if let Some(l) = ll {
            info!("Setting log level to {:?}", l);
            ffmpeg::log::set_level(l);
        }

        let frame_received_ts = i64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .expect("Milliseconds must fit i64");

        if let Some(index) = audio_stream_index_opt {
            if index == stream.index() {
                if let Some(name) = audio_opt
                    .as_ref()
                    .and_then(|a| a.codec().as_ref().map(|c| String::from(c.name())))
                {
                    debug!("Audio streams are not supported yet. Codec is {}", name);
                }
            }
        }

        if stream.index() == video_stream_index {
            let p = &packet;
            let time_base_r = stream.time_base();

            let has_key_frames = match is_stream_key_framed(stream.codec().id()) {
                Ok(res) => res,
                Err(e) => {
                    panic!(
                        "Unsupported video codec detected: {:?}, exit the application.",
                        e
                    );
                }
            };

            if has_key_frames {
                if p.is_key() {
                    skip_until_first_key_frame = false;
                }
            } else {
                skip_until_first_key_frame = false;
            }

            if skip_until_first_key_frame {
                continue;
            }

            let decode = decode
                || (autoconvert_raw_formats_to_rgb24
                    && video_decoder.codec().map(|c| c.id()) == Some(Id::RAWVIDEO));

            let raw_frames = if decode {
                let mut raw_frames = Vec::new();
                video_decoder
                    .send_packet(p)
                    .expect("Packet must be sent to decoder");
                let mut decoded = Video::empty();
                while video_decoder.receive_frame(&mut decoded).is_ok() {
                    let mut rgb_frame = Video::empty();
                    converter
                        .run(&decoded, &mut rgb_frame)
                        .expect("RGB conversion must succeed");

                    raw_frames.push((
                        rgb_frame.data(0).to_vec(),
                        rgb_frame.stride(0) as u32 / DECODED_PIX_BYTES,
                        rgb_frame.plane_height(0),
                    ));
                }
                raw_frames
            } else {
                vec![(
                    p.data().unwrap().to_vec(),
                    video_decoder.width(),
                    video_decoder.height(),
                )]
            };

            for (raw_frame, width, height) in raw_frames {
                let codec = if !decode {
                    String::from(video_decoder.codec().unwrap().name())
                } else {
                    String::from(Id::RAWVIDEO.name())
                };

                let pixel_format = if !decode {
                    format!("{:?}", video_decoder.format())
                } else {
                    format!("{:?}", DECODING_FORMAT)
                };

                let key_frame = p.is_key();
                let pts = p.pts();
                let dts = p.dts();
                let corrupted = p.is_corrupt();
                let fps = stream.rate().to_string();
                let avg_fps = stream.avg_frame_rate().to_string();

                debug!("Frame info: codec_name={:?}, FPS={:?}, AVG_FPS={:?}, width={}, height={}, is_key={}, len={}, pts={:?}, dts={:?}, is_corrupt={}, pixel_format={}",
                         codec, fps, avg_fps, width, height, key_frame, raw_frame.len(),
                         pts, dts, corrupted, pixel_format);

                let frame_processed_ts = i64::try_from(
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_millis(),
                )
                .expect("Milliseconds must fit i64");

                let frame_envelope = VideoFrameEnvelope {
                    codec,
                    frame_width: i64::from(width),
                    frame_height: i64::from(height),
                    key_frame,
                    pts,
                    dts,
                    corrupted,
                    time_base: (time_base_r.0 as i64, time_base_r.1 as i64),
                    fps,
                    avg_fps,
                    pixel_format,
                    queue_full_skipped_count,
                    payload: raw_frame,
                    frame_received_ts,
                    frame_processed_ts,
                    queue_len: i64::try_from(tx.len()).unwrap(),
                };

                if !block_if_queue_full {
                    if !tx.is_full() {
                        let res = tx.send(frame_envelope);

                        if let Err(e) = res {
                            error!("Unable to send data to upstream. Error is: {:?}", e);
                            break;
                        }
                    } else {
                        dbg!("Sink queue is full, the frame is skipped.");
                        queue_full_skipped_count += 1;
                    }
                } else {
                    tx.send(frame_envelope)
                        .expect("Unable to send data to upstream");
                }
            }
        }
    }
}

fn assign_log_level(ffmpeg_log_level: FFmpegLogLevel) -> Level {
    match ffmpeg_log_level {
        FFmpegLogLevel::Error => Level::Error,
        FFmpegLogLevel::Warn => Level::Warning,
        FFmpegLogLevel::Info => Level::Info,
        FFmpegLogLevel::Debug => Level::Debug,
        FFmpegLogLevel::Trace => Level::Trace,
        FFmpegLogLevel::Quiet => Level::Quiet,
        FFmpegLogLevel::Panic => Level::Panic,
        FFmpegLogLevel::Fatal => Level::Fatal,
    }
}

#[pymethods]
impl FFMpegSource {
    #[new]
    #[pyo3(signature = (uri, params,
        queue_len = 32,
        decode = false,
        autoconvert_raw_formats_to_rgb24 = false,
        block_if_queue_full = false,
        ffmpeg_log_level = FFmpegLogLevel::Info)
    )]
    pub fn new(
        uri: String,
        params: HashMap<String, String>,
        queue_len: i64,
        decode: bool,
        autoconvert_raw_formats_to_rgb24: bool,
        block_if_queue_full: bool,
        ffmpeg_log_level: FFmpegLogLevel,
    ) -> Self {
        assert!(queue_len > 0, "Queue length must be a positive number");

        let (tx, video_source) = crossbeam::channel::bounded(
            usize::try_from(queue_len).expect("Unable to get queue length from the argument"),
        );
        let exit_signal = Arc::new(Mutex::new(false));
        let log_level = Arc::new(Mutex::new(Some(assign_log_level(ffmpeg_log_level))));

        let thread_exit_signal = exit_signal.clone();
        let thread_ll = log_level.clone();
        let thread = Some(spawn(move || {
            handle(
                uri,
                params,
                tx,
                thread_exit_signal,
                decode,
                autoconvert_raw_formats_to_rgb24,
                block_if_queue_full,
                thread_ll,
            )
        }));

        Self {
            video_source,
            thread,
            exit_signal,
            log_level,
        }
    }

    pub fn video_frame(&self) -> PyResult<VideoFrameEnvelope> {
        Python::with_gil(|py| {
            py.allow_threads(|| {
                let res = self.video_source.recv();
                match res {
                    Err(e) => Err(PyBrokenPipeError::new_err(format!("{:?}", e))),
                    Ok(x) => Ok(x),
                }
            })
        })
    }

    #[setter]
    pub fn log_level(&self, ffmpeg_log_level: FFmpegLogLevel) {
        let mut ll = self.log_level.lock();
        *ll = Some(assign_log_level(ffmpeg_log_level));
    }
}

#[pymodule]
#[pyo3(name = "ffmpeg_input")]
fn ffmpeg_input(_py: Python, m: &PyModule) -> PyResult<()> {
    _ = env_logger::try_init_from_env("LOGLEVEL").map_err(|e| {
        log::warn!("Unable to initialize logger. Error is: {:?}", e);
    });
    m.add_class::<VideoFrameEnvelope>()?;
    m.add_class::<FFMpegSource>()?;
    m.add_class::<FFmpegLogLevel>()?;
    Ok(())
}
