use anyhow::bail;
use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};
use std::time::{Instant, SystemTime};

use crossbeam::channel::{Receiver, Sender};
use derive_builder::Builder;
use ffmpeg::util::frame::video::Video;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::{Id, Parameters};
use ffmpeg_next::ffi::{av_bsf_alloc, av_bsf_init, AVBSFContext, AVERROR, AVERROR_EOF};
use ffmpeg_next::format::{input_with_dictionary, Pixel};
use ffmpeg_next::log::Level;
use ffmpeg_next::packet::Mut;
use ffmpeg_next::software::converter;
use ffmpeg_next::sys::{
    av_bsf_get_by_name, av_bsf_receive_packet, av_bsf_send_packet, av_opt_set,
    avcodec_parameters_copy, AV_OPT_SEARCH_CHILDREN, EAGAIN,
};
use ffmpeg_next::{Packet, Rational};
use log::{debug, error, info};
use parking_lot::Mutex;
use pyo3::exceptions::{PyBrokenPipeError, PySystemError, PyValueError};
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

#[pyclass]
#[derive(Clone, Debug)]
pub struct BsfFilter {
    codec: String,
    name: String,
    params: Vec<(String, String)>,
}

#[pymethods]
impl BsfFilter {
    #[new]
    #[pyo3(signature = (codec, name, params = vec![]))]
    fn new(codec: String, name: String, params: Vec<(String, String)>) -> Self {
        Self {
            codec,
            name,
            params,
        }
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

#[pyclass(eq, eq_int)]
#[derive(Debug, Clone, PartialEq)]
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
        let bytes = PyBytes::new_bound_with(py, self.payload.len(), |b: &mut [u8]| {
            b.copy_from_slice(&self.payload);
            Ok(())
        })?;
        Ok(PyObject::from(bytes))
    }
}

#[pyclass]
pub struct FFMpegSource {
    video_source: Receiver<VideoFrameEnvelope>,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
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
        let _ = t
            .unwrap()
            .join()
            .expect("Thread must be finished normally")
            .map_err(|e| {
                error!("Error in the worker thread. Error is: {:?}", e);
            });
        debug!("Worker thread is terminated");
    }
}

fn handle_wrapper(params: HandleParams) -> anyhow::Result<()> {
    let exit_signal = params.exit_signal.clone();
    match handle(params) {
        Ok(_) => Ok(()),
        Err(e) => {
            let mut state = exit_signal.lock();
            *state = true;
            error!("Error in the worker thread. Error is: {:?}", e);
            Err(e)
        }
    }
}

#[derive(Builder)]
struct HandleParams {
    uri: String,
    params: Vec<(String, String)>,
    tx: Sender<VideoFrameEnvelope>,
    init_complete: Sender<()>,
    exit_signal: Arc<Mutex<bool>>,
    decode: bool,
    autoconvert_raw_formats_to_rgb24: bool,
    block_if_queue_full: bool,
    log_level: Arc<Mutex<Option<Level>>>,
    bsf_filters: Vec<BsfFilter>,
}

struct BitStreamFilterContext {
    ptr: *mut AVBSFContext,
}

impl BitStreamFilterContext {
    pub fn name(&self) -> String {
        unsafe {
            let ptr = (*self.ptr).filter.as_ref().unwrap().name;
            let c_str = std::ffi::CStr::from_ptr(ptr);
            c_str.to_string_lossy().to_string()
        }
    }
}

fn init_bsf(
    name: &str,
    video_parameters: &Parameters,
    time_base: Rational,
    opts: &[(String, String)],
) -> anyhow::Result<BitStreamFilterContext> {
    unsafe {
        let c_name = CString::new(name).unwrap();
        let ptr = av_bsf_get_by_name(c_name.as_ptr());

        if ptr.is_null() {
            bail!("Unable to find bitstream filter by name: {}", name);
        }

        let mut av_bsf_ctx = std::ptr::null_mut();
        if av_bsf_alloc(ptr, &mut av_bsf_ctx) < 0 {
            bail!("Unable to allocate bitstream filter context");
        }

        if avcodec_parameters_copy((*av_bsf_ctx).par_in, video_parameters.as_ptr()) < 0 {
            bail!("Unable to copy codec parameters");
        }

        (*av_bsf_ctx).time_base_in = time_base.into();

        for (ok, ov) in opts {
            let opt_k = CString::new(ok.as_str()).unwrap();
            let opt_v = CString::new(ov.as_str()).unwrap();
            av_opt_set(
                av_bsf_ctx as *mut _,
                opt_k.as_ptr(),
                opt_v.as_ptr(),
                AV_OPT_SEARCH_CHILDREN,
            );
            // Insert AUD
        }

        if av_bsf_init(av_bsf_ctx) < 0 {
            bail!("Unable to initialize bitstream filter context");
        }

        Ok(BitStreamFilterContext { ptr: av_bsf_ctx })
    }
}

fn process_bsf(
    filters: &mut Vec<BitStreamFilterContext>,
    packet: &ffmpeg::Packet,
) -> anyhow::Result<Vec<Packet>> {
    let mut packets = vec![packet.clone()];
    for filter in filters {
        debug!("Filter: {}", filter.name());
        debug!("Ingress packet count: {}", packets.len());
        let mut new_packets = Vec::new();
        for mut packet in packets.drain(..) {
            unsafe {
                if av_bsf_send_packet(filter.ptr, packet.as_mut_ptr()) < 0 {
                    error!("Unable to send packet to bitstream filter");
                }

                loop {
                    let mut new_packet = Packet::new(packet.size() + 2048);
                    let ret = av_bsf_receive_packet(filter.ptr, packet.as_mut_ptr());
                    if ret < 0 {
                        break;
                    }
                    if ret == AVERROR(EAGAIN) || ret == AVERROR_EOF {
                        break;
                    }
                    new_packet.set_stream(packet.stream());
                    new_packet.set_flags(packet.flags());
                    new_packet.set_dts(packet.dts());
                    new_packet.set_pts(packet.pts());
                    new_packet.set_duration(packet.duration());
                    new_packets.push(new_packet.clone());
                }
            }
        }
        debug!("Egress packet count: {}", new_packets.len());
        packets = new_packets;
    }

    Ok(packets)
}

#[allow(clippy::too_many_arguments)]
fn handle(params: HandleParams) -> anyhow::Result<()> {
    let mut queue_full_skipped_count = 0;
    let now = Instant::now();
    ffmpeg::init().map_err(|e| {
        error!("Unable to initialize FFmpeg. Error is: {:?}", e);
        e
    })?;

    let ll = params.log_level.lock().take();

    if let Some(l) = ll {
        info!("Setting log level to {:?}", l);
        ffmpeg::log::set_level(l);
    }

    let mut opts = ffmpeg::Dictionary::new();
    for (k, v) in &params.params {
        opts.set(k, v);
    }
    let p = Path::new(params.uri.as_str());

    let mut ictx = input_with_dictionary(&p, opts).map_err(|e| {
        error!("Unable to open input URI. Error is: {:?}", e);
        e
    })?;

    let video_input = match ictx.streams().best(ffmpeg_next::media::Type::Video) {
        Some(s) => s,
        None => {
            let msg = "Cannot discover the best suitable video stream to work with.";
            error!("{}", msg);
            bail!(msg);
        }
    };
    let video_parameters = video_input.parameters();
    let time_base = video_input.time_base();

    info!("Codec: {:?}", video_input.codec().id());

    let video_stream_index = video_input.index();

    let mut video_filters = Vec::new();

    let codec_name = video_input.codec().id().name();
    for f in &params.bsf_filters {
        if f.codec != codec_name {
            info!(
                "Skipping filter {} as it is not applicable to codec {}, must match {}",
                f.name, codec_name, f.codec
            );
            continue;
        }

        info!(
            "Initializing filter: {} with parameters {:?}",
            f.name, f.params
        );

        video_filters.push(init_bsf(
            f.name.as_str(),
            &video_parameters,
            time_base,
            &f.params,
        )?);
    }

    let mut video_decoder =
        ffmpeg::codec::context::Context::from_parameters(video_input.parameters())
            .and_then(|c| c.decoder().video())
            .map_err(|e| {
                error!("Unable to get video decoder. Error is: {:?}", e);
                e
            })?;

    let mut converter = converter(
        (video_decoder.width(), video_decoder.height()),
        video_decoder.format(),
        DECODING_FORMAT,
    )
    .map_err(|e| {
        error!("Unable to get video converter. Error is: {:?}", e);
        e
    })?;

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
    params.init_complete.send(()).map_err(|e| {
        error!("Unable to send init complete signal. Error is: {:?}", e);
        e
    })?;
    info!(
        "FFmpeg is initialized for URI: {}, elapsed: {:?}",
        params.uri,
        now.elapsed()
    );

    for (stream, packet) in ictx.packets() {
        if *params.exit_signal.lock() {
            break;
        }
        let ll = params.log_level.lock().take();

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
        .map_err(|e| {
            error!("Unable to get current time. Error is: {:?}", e);
            e
        })?;

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
            let modified_packets = process_bsf(&mut video_filters, &packet)?;
            for p in &modified_packets {
                let time_base_r = stream.time_base();

                let has_key_frames = match is_stream_key_framed(stream.codec().id()) {
                    Ok(res) => res,
                    Err(e) => {
                        bail!(
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
                    debug!("Skipping until the first key frame");
                    continue;
                }

                let decode = params.decode
                    || (params.autoconvert_raw_formats_to_rgb24
                        && video_decoder.codec().map(|c| c.id()) == Some(Id::RAWVIDEO));

                let raw_frames = if decode {
                    let mut raw_frames = Vec::new();
                    video_decoder.send_packet(p).map_err(|e| {
                        error!("Unable to send packet to decoder. Error is: {:?}", e);
                        e
                    })?;
                    let mut decoded = Video::empty();
                    while video_decoder.receive_frame(&mut decoded).is_ok() {
                        let mut rgb_frame = Video::empty();
                        converter.run(&decoded, &mut rgb_frame).map_err(|e| {
                            error!("Unable to convert frame to RGB. Error is: {:?}", e);
                            e
                        })?;
                        raw_frames.push((
                            rgb_frame.data(0).to_vec(),
                            rgb_frame.stride(0) as u32 / DECODED_PIX_BYTES,
                            rgb_frame.plane_height(0),
                        ));
                    }
                    raw_frames
                } else {
                    vec![(
                        p.data().unwrap_or(&[]).to_vec(),
                        video_decoder.width(),
                        video_decoder.height(),
                    )]
                };

                for (raw_frame, width, height) in raw_frames {
                    let codec = if !decode {
                        match video_decoder.codec() {
                            Some(c) => String::from(c.name()),
                            None => bail!("Unable to get codec name"),
                        }
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
                    .map_err(|e| {
                        error!("Unable to get current time. Error is: {:?}", e);
                        e
                    })?;

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
                        queue_len: i64::try_from(params.tx.len()).unwrap(),
                    };

                    if !params.block_if_queue_full {
                        if !params.tx.is_full() {
                            let res = params.tx.send(frame_envelope);

                            if let Err(e) = res {
                                error!("Unable to send data to upstream. Error is: {:?}", e);
                                break;
                            }
                        } else {
                            dbg!("Sink queue is full, the frame is skipped.");
                            queue_full_skipped_count += 1;
                        }
                    } else {
                        params.tx.send(frame_envelope).map_err(|e| {
                            error!("Unable to send data to upstream. Error is: {:?}", e);
                            e
                        })?;
                    }
                }
            }
        }
    }
    Ok(())
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
    #[allow(clippy::too_many_arguments)]
    #[new]
    #[pyo3(signature = (uri, params,
        queue_len = 32,
        decode = false,
        autoconvert_raw_formats_to_rgb24 = false,
        block_if_queue_full = false,
        init_timeout_ms = 10000,
        ffmpeg_log_level = FFmpegLogLevel::Info,
        bsf_filters = vec![])
    )]
    pub fn new(
        uri: String,
        params: Vec<(String, String)>,
        queue_len: i64,
        decode: bool,
        autoconvert_raw_formats_to_rgb24: bool,
        block_if_queue_full: bool,
        init_timeout_ms: u64,
        ffmpeg_log_level: FFmpegLogLevel,
        bsf_filters: Vec<BsfFilter>,
    ) -> PyResult<Self> {
        assert!(queue_len > 0, "Queue length must be a positive number");

        let (tx, video_source) = crossbeam::channel::bounded(
            usize::try_from(queue_len).map_err(|e| PySystemError::new_err(format!("{:?}", e)))?,
        );

        let (init_tx, init_rx) = crossbeam::channel::bounded(1);

        let exit_signal = Arc::new(Mutex::new(false));
        let log_level = Arc::new(Mutex::new(Some(assign_log_level(ffmpeg_log_level))));

        let handle_params = HandleParamsBuilder::default()
            .uri(uri.clone())
            .params(params.into_iter().collect())
            .tx(tx)
            .init_complete(init_tx)
            .exit_signal(exit_signal.clone())
            .decode(decode)
            .autoconvert_raw_formats_to_rgb24(autoconvert_raw_formats_to_rgb24)
            .block_if_queue_full(block_if_queue_full)
            .log_level(log_level.clone())
            .bsf_filters(bsf_filters.clone())
            .build()
            .map_err(|e| {
                error!("Unable to create handle params. Error is: {:?}", e);
                PyValueError::new_err(format!("{:?}", e))
            })?;

        let thread = Some(spawn(move || handle_wrapper(handle_params)));

        init_rx
            .recv_timeout(std::time::Duration::from_millis(init_timeout_ms))
            .map_err(|e| {
                error!("Unable to initialize the worker thread. Error is: {:?}", e);
                PySystemError::new_err(format!("{:?}", e))
            })?;

        Ok(Self {
            video_source,
            thread,
            exit_signal,
            log_level,
        })
    }

    pub fn stop(&self) {
        let mut exit_signal = self.exit_signal.lock();
        *exit_signal = true;
    }

    #[getter]
    pub fn is_running(&self) -> bool {
        !*self.exit_signal.lock()
    }

    #[pyo3(signature = (timeout_ms = 10000))]
    pub fn video_frame(&self, timeout_ms: usize) -> PyResult<VideoFrameEnvelope> {
        if *self.exit_signal.lock() {
            return Err(PySystemError::new_err("Worker thread is not running"));
        }
        Python::with_gil(|py| {
            py.allow_threads(|| {
                let res = self
                    .video_source
                    .recv_timeout(std::time::Duration::from_millis(
                        u64::try_from(timeout_ms).map_err(|e| {
                            error!("Unable to convert timeout to u64. Error is: {:?}", e);
                            e
                        })?,
                    ));
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
fn ffmpeg_input(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    _ = env_logger::try_init_from_env("LOGLEVEL").map_err(|e| {
        log::warn!("Unable to initialize logger. Error is: {:?}", e);
    });
    m.add_class::<VideoFrameEnvelope>()?;
    m.add_class::<FFMpegSource>()?;
    m.add_class::<FFmpegLogLevel>()?;
    m.add_class::<BsfFilter>()?;
    Ok(())
}
