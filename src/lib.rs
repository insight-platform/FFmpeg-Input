use crossbeam::channel::{Receiver, Sender};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::Id;
use ffmpeg_next::format::input_with_dictionary;
use log::{debug, error};
use pyo3::exceptions::PyBrokenPipeError;
use pyo3::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{spawn, JoinHandle};

fn is_stream_key_framed(id: ffmpeg::codec::Id) -> Result<bool, String> {
    let mut keyframed = Some(true);

    match id {
        Id::H264 => {}
        Id::H265 => {}
        Id::HEVC => {}
        Id::VP9 => {}
        Id::AV1 => {}
        Id::MPEG1VIDEO => {}
        Id::MPEG2VIDEO => {}
        Id::MPEG4 => {}
        Id::MSMPEG4V1 => {}
        Id::MSMPEG4V2 => {}
        Id::MSMPEG4V3 => {}
        Id::THEORA => {}
        Id::FLV1 => {}
        Id::MJPEG => keyframed = Some(false),
        Id::RAWVIDEO => keyframed = Some(false),
        _ => keyframed = None,
    };

    match keyframed {
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
) {
    ffmpeg::init().unwrap();
    let mut opts = ffmpeg::Dictionary::new();
    for kv in &params {
        opts.set(kv.0.as_str(), kv.1.as_str());
    }
    let p = Path::new(uri.as_str());

    let mut ictx = input_with_dictionary(&p, opts).unwrap();

    let video_input = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap_or_else(|| panic!("Cannot detect the best suitable video stream to work with."));

    let video_stream_index = video_input.index();

    let video_decoder = ffmpeg::codec::context::Context::from_parameters(video_input.parameters())
        .and_then(|c| c.decoder().video())
        .unwrap();

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
        if *signal.lock().unwrap() {
            break;
        }

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

            let codec = String::from(video_decoder.codec().unwrap().name());
            let frame_width = video_decoder.width();
            let frame_height = video_decoder.height();
            let pixel_format = format!("{:?}", video_decoder.format());

            let key_frame = p.is_key();
            let pts = p.pts();
            let dts = p.dts();
            let corrupted = p.is_corrupt();
            let fps = stream.rate().to_string();
            let avg_fps = stream.avg_frame_rate().to_string();

            debug!("Frame info: codec_name={:?}, FPS={:?}, AVG_FPS={:?}, width={}, height={}, is_key={}, len={}, pts={:?}, dts={:?}, is_corrupt={}, pixel_format={}",
                         codec, fps, avg_fps, frame_width, frame_height, key_frame, p.data().unwrap().len(),
                         pts, dts, corrupted, pixel_format);

            if tx.is_empty() {
                let res = tx.send(VideoFrameEnvelope {
                    codec,
                    frame_width: i64::from(frame_width),
                    frame_height: i64::from(frame_height),
                    key_frame,
                    pts,
                    dts,
                    corrupted,
                    fps,
                    avg_fps,
                    pixel_format,
                    payload: p.data().unwrap().to_vec(),
                });

                if let Err(e) = res {
                    error!("Unable to send data to upstream. Error is: {:?}", e);
                    break;
                }
            }
        }
    }
}

#[pymethods]
impl FFMpegSource {
    #[new]
    pub fn new(uri: String, params: HashMap<String, String>) -> Self {
        let _r = env_logger::try_init();
        let (tx, video_source) = crossbeam::channel::bounded(1);
        let exit_signal = Arc::new(Mutex::new(false));
        let thread_exit_signal = exit_signal.clone();
        let thread = Some(spawn(move || handle(uri, params, tx, thread_exit_signal)));
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
