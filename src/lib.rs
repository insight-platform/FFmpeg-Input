use crossbeam::channel::{Receiver, Sender};
use ffmpeg::util::frame::video::Video;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::Id;
use ffmpeg_next::format::{input_with_dictionary, Pixel};
use ffmpeg_next::software::converter;
use log::{debug, error, warn};
use pyo3::exceptions::PyBrokenPipeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};
use std::time::SystemTime;

fn is_stream_key_framed(id: ffmpeg::codec::Id) -> Result<bool, String> {
    let key_frames = match id {
        Id::H264
        | Id::H265
        | Id::HEVC
        | Id::VP9
        | Id::AV1
        | Id::MPEG1VIDEO
        | Id::MPEG2VIDEO
        | Id::MPEG4
        | Id::MSMPEG4V1
        | Id::MSMPEG4V2
        | Id::MSMPEG4V3
        | Id::THEORA
        | Id::FLV1 => Some(true),
        Id::MJPEG | Id::RAWVIDEO => Some(false),
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

    fn payload_as_bytes(&self, py: Python) -> PyObject {
        PyBytes::new(py, &self.payload).into()
    }
}

#[pyclass]
pub struct FFMpegSource {
    video_source: Receiver<VideoFrameEnvelope>,
    thread: Option<JoinHandle<()>>,
    exit_signal: Arc<Mutex<bool>>,
}

impl Drop for FFMpegSource {
    fn drop(&mut self) {
        {
            let mut exit_signal = self
                .exit_signal
                .lock()
                .expect("Exit mutex must be always locked without problems");
            *exit_signal = true;
        }
        let t = self.thread.take();
        t.unwrap().join().expect("Thread must be finished normally");
        debug!("Worker thread is terminated");
    }
}

fn handle(
    uri: String,
    params: HashMap<String, String>,
    tx: Sender<VideoFrameEnvelope>,
    signal: Arc<Mutex<bool>>,
    decode: bool,
) {
    let mut queue_full_skipped_count = 0;
    ffmpeg::init().expect("FFmpeg initialization must be successful");
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
        Pixel::BGR24,
    )
    .expect("Video scaler must be initialized");

    // let mut video_scaler = Context::get(
    //     video_decoder.format(),
    //     video_decoder.width(),
    //     video_decoder.height(),
    //     Pixel::BGR24,
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
        if *signal.lock().expect("Mutex is poisoned. Critical error.") {
            break;
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
                        rgb_frame.stride(0) as u32 / 3,
                        rgb_frame.plane_height(0) as u32,
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
                let codec = String::from(video_decoder.codec().unwrap().name());
                let pixel_format = format!("{:?}", video_decoder.format());

                let key_frame = p.is_key();
                let pts = p.pts();
                let dts = p.dts();
                let corrupted = p.is_corrupt();
                let fps = stream.rate().to_string();
                let avg_fps = stream.avg_frame_rate().to_string();

                debug!("Frame info: codec_name={:?}, FPS={:?}, AVG_FPS={:?}, width={}, height={}, is_key={}, len={}, pts={:?}, dts={:?}, is_corrupt={}, pixel_format={}",
                         codec, fps, avg_fps, width, height, key_frame, raw_frame.len(),
                         pts, dts, corrupted, pixel_format);

                if !tx.is_full() {
                    let frame_processed_ts = i64::try_from(
                        SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_millis(),
                    )
                    .expect("Milliseconds must fit i64");

                    let res = tx.send(VideoFrameEnvelope {
                        codec,
                        frame_width: i64::from(width),
                        frame_height: i64::from(height),
                        key_frame,
                        pts,
                        dts,
                        corrupted,
                        fps,
                        avg_fps,
                        pixel_format,
                        queue_full_skipped_count,
                        payload: raw_frame,
                        frame_received_ts,
                        frame_processed_ts,
                        queue_len: i64::try_from(tx.len()).unwrap(),
                    });

                    if let Err(e) = res {
                        error!("Unable to send data to upstream. Error is: {:?}", e);
                        break;
                    }
                } else {
                    warn!("Sink queue is full, the frame is skipped.");
                    queue_full_skipped_count += 1;
                }
            }
        }
    }
}

#[pymethods]
impl FFMpegSource {
    #[new]
    pub fn new(uri: String, params: HashMap<String, String>, len: i64, decode: bool) -> Self {
        assert!(len > 0, "Queue length must be a positive number");
        let _r = env_logger::try_init();
        let (tx, video_source) = crossbeam::channel::bounded(
            usize::try_from(len).expect("Unable to get queue length from the argument"),
        );
        let exit_signal = Arc::new(Mutex::new(false));
        let thread_exit_signal = exit_signal.clone();
        let thread = Some(spawn(move || {
            handle(uri, params, tx, thread_exit_signal, decode)
        }));
        Self {
            video_source,
            thread,
            exit_signal,
        }
    }

    pub fn video_frame(&self) -> PyResult<VideoFrameEnvelope> {
        let res = self.video_source.recv();
        match res {
            Err(e) => Err(PyBrokenPipeError::new_err(format!("{:?}", e))),
            Ok(x) => Ok(x),
        }
    }
}

mod python {
    use crate::{FFMpegSource, VideoFrameEnvelope};
    use pyo3::prelude::*;

    #[pymodule]
    #[pyo3(name = "ffmpeg_input")]
    fn ffmpeg_input(_py: Python, m: &PyModule) -> PyResult<()> {
        m.add_class::<VideoFrameEnvelope>()?;
        m.add_class::<FFMpegSource>()?;
        Ok(())
    }
}
