use error_chain::ChainedError;
use ffmpeg::{Rational, format};
use ffmpeg::frame::Video as VideoFrame;
use std::ops::Deref;
use std::sync::{Mutex, ONCE_INIT, Once, RwLock};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;

use encode::{Encoder, EncoderParameters};
use engine::Engine;
use errors::*;
use fps_converter::*;
use hooks::hw;
// use profiler::*;

lazy_static! {
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);

    /// Receives buffers to write pixels to.
    static ref VIDEO_BUF_RECEIVER: Mutex<Option<Receiver<VideoBuffer>>> = Mutex::new(None);

    /// Receives buffers to write samples to.
    static ref AUDIO_BUF_RECEIVER: Mutex<Option<Receiver<AudioBuffer>>> = Mutex::new(None);

    /// Receives various game thread-related events, such as console messages to print.
    static ref GAME_THREAD_RECEIVER: Mutex<Option<Receiver<GameThreadEvent>>> = Mutex::new(None);

    /// Sends events and frames to encode to the capture thread.
    static ref SEND_TO_CAPTURE_THREAD: Mutex<Option<Sender<CaptureThreadEvent>>> = Mutex::new(None);
}

thread_local! {
    // pub static GAME_THREAD_PROFILER: RefCell<Option<Profiler>> = RefCell::new(None);
    // pub static AUDIO_PROFILER: RefCell<Option<Profiler>> = RefCell::new(None);
    // pub static CAPTURE_THREAD_PROFILER: RefCell<Option<Profiler>> = RefCell::new(None);
}

pub struct CaptureParameters {
    pub sampling_exposure: f64,
    pub sampling_time_base: Option<Rational>,
    pub sound_extra: f64,
    pub time_base: Rational,
    pub volume: f32,
}

enum CaptureThreadEvent {
    CaptureStart(EncoderParameters),
    CaptureStop,
    VideoFrame((VideoBuffer, usize)),
    AudioFrame(AudioBuffer),
}

pub enum GameThreadEvent {
    Message(String),
    EncoderPixelFormat(format::Pixel),
}

pub struct VideoBuffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: format::Pixel,
    components: u8,
    frame: VideoFrame,
    data_is_in_frame: bool,
}

pub struct AudioBuffer {
    data: Vec<(i16, i16)>,
}

struct SendOnDrop<'a, T: 'a> {
    buffer: Option<T>,
    channel: &'a Sender<T>,
}

impl VideoBuffer {
    #[inline]
    fn new() -> Self {
        Self {
            data: Vec::new(),
            width: 0,
            height: 0,
            format: format::Pixel::RGB24,
            components: format::Pixel::RGB24.descriptor().unwrap().nb_components(),
            frame: VideoFrame::empty(),
            data_is_in_frame: false,
        }
    }

    #[inline]
    pub fn set_resolution(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            println!("Changing resolution from {}×{} to {}×{}.",
                     self.width,
                     self.height,
                     width,
                     height);

            self.width = width;
            self.height = height;
        }
    }

    #[inline]
    pub fn set_format(&mut self, format: format::Pixel) {
        if self.format != format {
            println!("Changing format from {:?} to {:?}", self.format, format);

            self.format = format;
            self.components = format.descriptor()
                                    .expect("invalid pixel format")
                                    .nb_components();
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.data_is_in_frame = false;
        self.data
            .resize((self.width * self.height * self.components as u32) as usize,
                    0);

        self.data.as_mut_slice()
    }

    pub fn get_frame(&mut self) -> &mut VideoFrame {
        self.data_is_in_frame = true;

        if self.width != self.frame.width() || self.height != self.frame.height() ||
            self.format != self.frame.format()
        {
            self.frame = VideoFrame::new(self.format, self.width, self.height);
        }

        &mut self.frame
    }

    pub fn copy_to_frame(&self, frame: &mut VideoFrame) {
        // Make sure the frame is of correct size.
        if self.width != frame.width() || self.height != frame.height() ||
            self.format != frame.format()
        {
            *frame = VideoFrame::new(self.format, self.width, self.height);
        }

        if self.data_is_in_frame {
            for i in 0..frame.planes() {
                frame.data_mut(i).copy_from_slice(self.frame.data(i));
            }
        } else {
            let mut offset = 0;
            let components_per_plane = if frame.planes() == 1 {
                self.components
            } else {
                1
            } as usize;

            for i in 0..frame.planes() {
                let stride = frame.stride(i);
                let plane_width = frame.plane_width(i) as usize;
                let plane_height = frame.plane_height(i) as usize;

                let mut plane_data = frame.data_mut(i);
                for y in 0..plane_height {
                    let plane_data_start = (plane_height - y - 1) * stride;
                    let length = plane_width * components_per_plane;

                    plane_data[plane_data_start..plane_data_start + length]
                        .copy_from_slice(&self.data[offset..offset + length]);

                    offset += length;
                }
            }
        }
    }
}

impl AudioBuffer {
    #[inline]
    fn new() -> Self {
        Self { data: Vec::new() }
    }

    #[inline]
    pub fn data(&self) -> &Vec<(i16, i16)> {
        &self.data
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut Vec<(i16, i16)> {
        &mut self.data
    }
}

impl<'a, T> SendOnDrop<'a, T> {
    #[inline]
    fn new(buffer: T, channel: &'a Sender<T>) -> Self {
        Self {
            buffer: Some(buffer),
            channel,
        }
    }
}

impl<'a, T> Drop for SendOnDrop<'a, T> {
    #[inline]
    fn drop(&mut self) {
        self.channel.send(self.buffer.take().unwrap()).unwrap();
    }
}

impl<'a, T> Deref for SendOnDrop<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.buffer.as_ref().unwrap()
    }
}

fn capture_thread(video_buf_sender: &Sender<VideoBuffer>,
                  audio_buf_sender: &Sender<AudioBuffer>,
                  event_sender: &Sender<GameThreadEvent>,
                  event_receiver: &Receiver<CaptureThreadEvent>) {
    // Send the buffers to the game thread right away.
    video_buf_sender.send(VideoBuffer::new()).unwrap();
    audio_buf_sender.send(AudioBuffer::new()).unwrap();

    // This is our frame which will only be reallocated on resolution changes.
    let mut frame = VideoFrame::empty();

    // This is set to true on encoding error or cap_stop and reset on cap_start.
    // When this is true, ignore any received frames.
    let mut drop_frames = true;

    // The encoder itself.
    let mut encoder: Option<Encoder> = None;

    // Event loop for the capture thread.
    loop {
        match event_receiver.recv().unwrap() {
            CaptureThreadEvent::CaptureStart(params) => {
                drop_frames = false;
                encoder = Encoder::start(&params)
                    .chain_err(|| {
                                   "could not start the encoder; check your terminal (Half-Life's \
                                    standard output) for ffmpeg messages"
                               })
                    .map_err(|ref e| {
                                 *CAPTURING.write().unwrap() = false;
                                 drop_frames = true;

                                 event_sender.send(GameThreadEvent::Message(format!("{}",
                                                                                    e.display())))
                                             .unwrap();
                             })
                    .ok();

                if let Some(ref encoder) = encoder {
                    event_sender.send(GameThreadEvent::EncoderPixelFormat(encoder.format()))
                                .unwrap();
                }
            }

            CaptureThreadEvent::CaptureStop => {
                stop_encoder(encoder.take(), event_sender);
                drop_frames = true;
            }

            CaptureThreadEvent::VideoFrame((buffer, times)) => {
                let buffer = SendOnDrop::new(buffer, &video_buf_sender);

                if drop_frames {
                    continue;
                }

                if let Err(e) = encode(&mut encoder, buffer, times, &mut frame) {
                    event_sender.send(GameThreadEvent::Message(format!("{}", e.display())))
                                .unwrap();

                    *CAPTURING.write().unwrap() = false;
                    stop_encoder(encoder.take(), event_sender);
                    drop_frames = true;
                }
            }

            CaptureThreadEvent::AudioFrame(buffer) => {
                let buffer = SendOnDrop::new(buffer, &audio_buf_sender);

                if drop_frames {
                    continue;
                }

                // Encode the audio.
                let result = encoder.as_mut().unwrap().take_audio(buffer.data());

                drop(audio_buf_sender);

                if let Err(e) = result {
                    event_sender.send(GameThreadEvent::Message(format!("{}", e.display())))
                                .unwrap();

                    *CAPTURING.write().unwrap() = false;
                    stop_encoder(encoder.take(), event_sender);
                    drop_frames = true;
                }
            }
        }
    }
}

fn encode(encoder: &mut Option<Encoder>,
          buf: SendOnDrop<VideoBuffer>,
          times: usize,
          frame: &mut VideoFrame)
          -> Result<()> {
    // Copy pixels into our video frame.
    buf.copy_to_frame(frame);

    // We're done with buf, now it can receive the next pack of pixels.
    drop(buf);

    let mut encoder = encoder.as_mut().unwrap();

    ensure!((frame.width(), frame.height()) == (encoder.width(), encoder.height()),
            "resolution changes are not supported");

    // Encode the frame.
    encoder.take(frame, times)
           .chain_err(|| "could not encode the frame")?;

    Ok(())
}

/// Properly closes and drops the encoder.
fn stop_encoder(encoder: Option<Encoder>, event_sender: &Sender<GameThreadEvent>) {
    if let Some(mut encoder) = encoder {
        if let Err(e) = encoder.finish() {
            event_sender.send(GameThreadEvent::Message(format!("{}", e.display())))
                        .unwrap();
        }

        drop(encoder);
    }
}

pub fn initialize(_: &Engine) {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        let (tx, rx) = channel::<VideoBuffer>();
        let (tx2, rx2) = channel::<AudioBuffer>();
        let (tx3, rx3) = channel::<GameThreadEvent>();
        let (tx4, rx4) = channel::<CaptureThreadEvent>();

        *VIDEO_BUF_RECEIVER.lock().unwrap() = Some(rx);
        *AUDIO_BUF_RECEIVER.lock().unwrap() = Some(rx2);
        *GAME_THREAD_RECEIVER.lock().unwrap() = Some(rx3);
        *SEND_TO_CAPTURE_THREAD.lock().unwrap() = Some(tx4);

        thread::spawn(move || capture_thread(&tx, &tx2, &tx3, &rx4));
    });
}

#[inline]
pub fn get_buffer(_: &Engine, (width, height): (u32, u32)) -> VideoBuffer {
    let mut buf = VIDEO_BUF_RECEIVER.lock()
                                    .unwrap()
                                    .as_ref()
                                    .unwrap()
                                    .recv()
                                    .unwrap();

    buf.set_resolution(width, height);

    buf
}

#[inline]
pub fn get_audio_buffer(_: &Engine) -> AudioBuffer {
    AUDIO_BUF_RECEIVER.lock()
                      .unwrap()
                      .as_ref()
                      .unwrap()
                      .recv()
                      .unwrap()
}

#[inline]
pub fn get_event(_: &Engine) -> Option<GameThreadEvent> {
    match GAME_THREAD_RECEIVER.lock()
                                .unwrap()
                                .as_ref()
                                .unwrap()
                                .try_recv() {
        Ok(event) => Some(event),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => unreachable!(),
    }
}

#[inline]
pub fn get_event_block(_: &Engine) -> GameThreadEvent {
    GAME_THREAD_RECEIVER.lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .recv()
                        .unwrap()
}

#[inline]
pub fn capture(_: &Engine, buf: VideoBuffer, times: usize) {
    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::VideoFrame((buf, times)))
                          .unwrap();
}

#[inline]
pub fn capture_audio(_: &Engine, buf: AudioBuffer) {
    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::AudioFrame(buf))
                          .unwrap();
}

#[inline]
pub fn is_capturing() -> bool {
    *CAPTURING.read().unwrap()
}

#[inline]
pub fn get_capture_parameters(engine: &Engine) -> &CaptureParameters {
    engine.data().capture_parameters.as_ref().unwrap()
}

pub fn stop(engine: &Engine) {
    if !is_capturing() {
        return;
    }

    hw::capture_remaining_sound(engine);

    *CAPTURING.write().unwrap() = false;
    if let Some(FPSConverters::Sampling(ref mut sampling_conv)) = engine.data().fps_converter {
        sampling_conv.free();
    }
    engine.data().fps_converter = None;
    engine.data().encoder_pixel_format = None;

    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::CaptureStop)
                          .unwrap();

    // GAME_THREAD_PROFILER.with(|p| if let Some(p) = p.borrow_mut().take() {
    //     if let Ok(data) = p.get_data() {
    //         let mut buf = format!("Captured {} frames. Game thread overhead: {:.3} msec:\n",
    //                               data.lap_count,
    //                               data.average_lap_time);
    //
    //         for &(section, time) in &data.average_section_times {
    //             buf.push_str(&format!("- {:.3} msec: {}\n", time, section));
    //         }
    //
    //         engine.con_print(&buf);
    //     }
    // });
    // AUDIO_PROFILER.with(|p| if let Some(p) = p.borrow_mut().take() {
    //                         if let Ok(data) = p.get_data() {
    //                             let mut buf = format!("Audio overhead: {:.3} msec:\n",
    //                                                   data.average_lap_time);
    //
    //                             for &(section, time) in &data.average_section_times {
    //                                 buf.push_str(&format!("- {:.3} msec: {}\n", time, section));
    //                             }
    //
    //                             engine.con_print(&buf);
    //                         }
    //                     });
}

/// Parses the given string and returns a time base.
///
/// The string can be in one of the two formats:
/// - `<i32 FPS>` - treated as an integer FPS value,
/// - `<i32 a> <i32 b>` - treated as a fractional `a/b` FPS value.
fn parse_fps(string: &str) -> Option<Rational> {
    if let Ok(fps) = string.parse() {
        if fps <= 0 {
            return None;
        }

        return Some((1, fps).into());
    }

    let mut split = string.splitn(2, ' ');
    if let Some(den) = split.next().and_then(|s| s.parse().ok()) {
        if let Some(num) = split.next().and_then(|s| s.parse().ok()) {
            if num <= 0 || den <= 0 {
                return None;
            }

            return Some((num, den).into());
        }
    }

    None
}

/// Parses the given string into a valid exposure value.
#[inline]
fn parse_exposure(string: &str) -> Result<f64> {
    string.parse()
          .chain_err(|| "could not convert the string to a floating point value")
          .and_then(|x| if x > 0f64 && x <= 1f64 {
        Ok(x)
    } else {
        bail!("allowed exposure values range from 0 (non-inclusive) to 1 (inclusive)")
    })
}

/// Parses the given string into a pixel format.
#[inline]
fn parse_pixel_format(string: &str) -> Result<format::Pixel> {
    if string.is_empty() {
        Ok(format::Pixel::None)
    } else {
        string.parse()
              .chain_err(|| "could not convert the string to a pixel format")
    }
}

macro_rules! to_string {
    ($engine:expr, $cvar:expr) => (
        $cvar.to_string($engine).chain_err(|| concat!("invalid ", stringify!($cvar)))?
    )
}

macro_rules! parse {
    ($engine:expr, $cvar:expr) => (
        $cvar.parse($engine).chain_err(|| concat!("invalid ", stringify!($cvar)))?
    );

    ($engine:expr, $cvar:expr, $type:ty) => (
        $cvar.parse::<$type>($engine).chain_err(|| concat!("invalid ", stringify!($cvar)))?
    )
}

/// Parses the CVar values into `EncoderParameters`.
#[inline]
fn parse_encoder_parameters(engine: &mut Engine) -> Result<EncoderParameters> {
    Ok(EncoderParameters {
           audio_bitrate: parse!(engine, cap_audio_bitrate, usize) * 1000,
           video_bitrate: parse!(engine, cap_video_bitrate, usize) * 1000,
           crf: to_string!(engine, cap_crf),
           filename: to_string!(engine, cap_filename),
           muxer_settings: to_string!(engine, cap_muxer_settings),
           pixel_format: parse_pixel_format(&to_string!(engine, cap_pixel_format))
               .chain_err(|| "invalid cap_pixel_format")?,
           preset: to_string!(engine, cap_x264_preset),
           time_base: parse_fps(&to_string!(engine, cap_fps))
               .ok_or("invalid cap_fps")?,
           audio_encoder_settings: to_string!(engine, cap_audio_encoder_settings),
           video_encoder_settings: to_string!(engine, cap_video_encoder_settings),
           vpx_threads: to_string!(engine, cap_vpx_threads),
           video_resolution: hw::get_resolution(&engine),
       })
}

/// Parses the CVar values into `CaptureParameters`.
#[inline]
fn parse_capture_parameters(engine: &mut Engine) -> Result<CaptureParameters> {
    Ok(CaptureParameters {
           sampling_exposure: parse_exposure(&to_string!(engine, cap_sampling_exposure))
               .chain_err(|| "invalid cap_sampling_exposure")?,
           sampling_time_base: parse_fps(&to_string!(engine, cap_sampling_sps)),
           sound_extra: parse!(engine, cap_sound_extra),
           time_base: parse_fps(&to_string!(engine, cap_fps))
               .ok_or("invalid cap_fps")?,
           volume: parse!(engine, cap_volume),
       })
}

/// Starts and stops the encoder.
fn test_encoder(parameters: &EncoderParameters) -> Result<()> {
    let mut encoder = Encoder::start(&parameters)
        .chain_err(|| {
                       "could not start the encoder; check your terminal (Half-Life's \
                        standard output) for ffmpeg messages"
                   })?;
    encoder.finish()
           .chain_err(|| "could not finish the encoder")?;
    Ok(())
}

command!(cap_start, |mut engine| {
    if is_capturing() {
        engine.con_print("Already capturing, please stop the capturing with cap_stop \
                          before starting it again.\n");
        return;
    }

    let parameters = match parse_encoder_parameters(&mut engine) {
        Ok(p) => p,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    engine.data().capture_parameters = match parse_capture_parameters(&mut engine) {
        Ok(p) => Some(p),
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    engine.data().fps_converter = if engine.data()
                                           .capture_parameters
                                           .as_ref()
                                           .unwrap()
                                           .sampling_time_base
                                           .is_some()
    {
        Some(FPSConverters::Sampling(SamplingConverter::new(&engine,
                                                            parameters.time_base.into(),
                                                            parameters.video_resolution)))
    } else {
        Some(FPSConverters::Simple(SimpleConverter::new(parameters.time_base.into())))
    };

    *CAPTURING.write().unwrap() = true;

    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::CaptureStart(parameters))
                          .unwrap();

    // GAME_THREAD_PROFILER.with(|p| *p.borrow_mut() = Some(Profiler::new()));
    // AUDIO_PROFILER.with(|p| *p.borrow_mut() = Some(Profiler::new()));

    hw::reset_sound_capture_remainder(&engine);
});

command!(cap_stop, |engine| {
    stop(&engine);
});

command!(cap_test, |mut engine| {
    let parameters = match parse_encoder_parameters(&mut engine) {
        Ok(p) => p,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    let _capture_parameters = match parse_capture_parameters(&mut engine) {
        Ok(p) => Some(p),
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    if let Err(ref e) = test_encoder(&parameters) {
        engine.con_print(&format!("{}", e.display()));
    } else {
        engine.con_print("Capture was started and stopped successfully.\n");
    }
});

// Encoder parameters.
cvar!(cap_video_bitrate, "0");
cvar!(cap_audio_bitrate, "256");
cvar!(cap_crf, "15");
cvar!(cap_filename, "capture.mp4");
cvar!(cap_fps, "60");
cvar!(cap_muxer_settings, "movflags=+faststart");
cvar!(cap_pixel_format, "");
cvar!(cap_audio_encoder_settings, "");
cvar!(cap_video_encoder_settings, "");
cvar!(cap_vpx_threads, "8");
cvar!(cap_x264_preset, "veryfast");

// Capture parameters.
cvar!(cap_sampling_exposure, "0.5");
cvar!(cap_sampling_sps, "");
cvar!(cap_sound_extra, "0");
cvar!(cap_volume, "0.4");
