use error_chain::ChainedError;
use ffmpeg::{Rational, format};
use ffmpeg::frame::Video as VideoFrame;
use std::cell::RefCell;
use std::ops::Deref;
use std::ptr;
use std::sync::{Mutex, ONCE_INIT, Once, RwLock};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;

use encode::{Encoder, EncoderParameters};
use engine::Engine;
use errors::*;
use hooks::hw;
use profiler::*;

lazy_static! {
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);

    /// Receives buffers to write pixels to.
    static ref VIDEO_BUF_RECEIVER: Mutex<Option<Receiver<VideoBuffer>>> = Mutex::new(None);

    /// Receives buffers to write samples to.
    static ref AUDIO_BUF_RECEIVER: Mutex<Option<Receiver<AudioBuffer>>> = Mutex::new(None);

    /// Receives messages to print to the game console.
    static ref MESSAGE_RECEIVER: Mutex<Option<Receiver<String>>> = Mutex::new(None);

    /// Sends events and frames to encode to the capture thread.
    static ref SEND_TO_CAPTURE_THREAD: Mutex<Option<Sender<CaptureThreadEvent>>> = Mutex::new(None);
}

thread_local! {
    pub static GAME_THREAD_PROFILER: RefCell<Option<Profiler>> = RefCell::new(None);
    pub static AUDIO_PROFILER: RefCell<Option<Profiler>> = RefCell::new(None);
    // pub static CAPTURE_THREAD_PROFILER: RefCell<Option<Profiler>> = RefCell::new(None);
}

pub struct CaptureParameters {
    pub sound_extra: f64,
    pub time_base: Rational,
    pub volume: f32,
}

enum CaptureThreadEvent {
    CaptureStart(EncoderParameters),
    CaptureStop,
    VideoFrame((VideoBuffer, f64)),
    AudioFrame(AudioBuffer),
}

pub struct VideoBuffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

pub struct AudioBuffer {
    data: Vec<(i16, i16)>,
}

struct SendOnDrop<'a, T: 'a> {
    buffer: Option<T>,
    channel: &'a Sender<T>,
}

impl VideoBuffer {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    pub fn set_resolution(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            println!("Changing resolution from {}×{} to {}×{}.",
                     self.width,
                     self.height,
                     width,
                     height);

            self.data.resize((width * height * 4) as usize, 0);
            self.width = width;
            self.height = height;
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.data.as_mut_slice()
    }

    pub fn copy_to_frame(&self, frame: &mut VideoFrame) {
        // Make sure frame is of correct size.
        if self.width != frame.width() || self.height != frame.height() {
            *frame = VideoFrame::new(format::Pixel::RGBA, self.width, self.height);
        }

        // Copy the pixel data into the frame.
        let stride = frame.stride(0) as u32;
        let mut data = frame.data_mut(0);

        for y in 0..self.height {
            unsafe {
                ptr::copy_nonoverlapping(self.data.as_ptr().offset((y * self.width * 4) as isize),
                                         data.as_mut_ptr()
                                             .offset(((self.height - y - 1) * stride) as isize),
                                         (self.width * 4) as usize);
            }
        }
    }
}

impl AudioBuffer {
    fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn data(&self) -> &Vec<(i16, i16)> {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Vec<(i16, i16)> {
        &mut self.data
    }
}

impl<'a, T> SendOnDrop<'a, T> {
    fn new(buffer: T, channel: &'a Sender<T>) -> Self {
        Self {
            buffer: Some(buffer),
            channel,
        }
    }
}

impl<'a, T> Drop for SendOnDrop<'a, T> {
    fn drop(&mut self) {
        self.channel.send(self.buffer.take().unwrap()).unwrap();
    }
}

impl<'a, T> Deref for SendOnDrop<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.buffer.as_ref().unwrap()
    }
}

fn capture_thread(video_buf_sender: &Sender<VideoBuffer>,
                  audio_buf_sender: &Sender<AudioBuffer>,
                  message_sender: &Sender<String>,
                  event_receiver: &Receiver<CaptureThreadEvent>) {
    // Send the buffers to the game thread right away.
    video_buf_sender.send(VideoBuffer::new()).unwrap();
    audio_buf_sender.send(AudioBuffer::new()).unwrap();

    // This is our frame which will only be reallocated on resolution changes.
    let mut frame = VideoFrame::empty();

    // This is set to true on encoding error or cap_stop and reset on cap_start.
    // When this is true, ignore any received frames.
    let mut drop_frames = true;

    // Encoding parameters, set on CaptureStart.
    let mut parameters = None;

    // The encoder itself.
    let mut encoder: Option<Encoder> = None;

    // Event loop for the capture thread.
    loop {
        match event_receiver.recv().unwrap() {
            CaptureThreadEvent::CaptureStart(params) => {
                drop_frames = false;
                parameters = Some(params);
            }

            CaptureThreadEvent::CaptureStop => {
                if let Some(mut encoder) = encoder.take() {
                    if let Err(e) = encoder.finish() {
                        message_sender.send(format!("{}", e.display())).unwrap();
                    }

                    // The encoder is dropped here.
                }

                drop_frames = true;
            }

            CaptureThreadEvent::VideoFrame((buffer, frametime)) => {
                let buffer = SendOnDrop::new(buffer, &video_buf_sender);

                if drop_frames {
                    continue;
                }

                if let Err(e) = encode(&mut encoder,
                                       buffer,
                                       frametime,
                                       parameters.as_ref().unwrap(),
                                       &mut frame)
                {
                    *CAPTURING.write().unwrap() = false;
                    drop_frames = true;

                    message_sender.send(format!("{}", e.display())).unwrap();
                }
            }

            CaptureThreadEvent::AudioFrame(buffer) => {
                let buffer = SendOnDrop::new(buffer, &audio_buf_sender);

                if drop_frames {
                    continue;
                }

                let result = encoder.as_mut().unwrap().take_audio(buffer.data());

                drop(audio_buf_sender);

                if let Err(e) = result {
                    *CAPTURING.write().unwrap() = false;
                    drop_frames = true;

                    message_sender.send(format!("{}", e.display())).unwrap();
                }
            }
        }
    }
}

fn encode(encoder: &mut Option<Encoder>,
          buf: SendOnDrop<VideoBuffer>,
          frametime: f64,
          parameters: &EncoderParameters,
          frame: &mut VideoFrame)
          -> Result<()> {
    // Copy pixels into our video frame.
    buf.copy_to_frame(frame);

    // We're done with buf, now it can receive the next pack of pixels.
    drop(buf);

    // If the encoder wasn't initialized, initialize it.
    if encoder.is_none() {
        *encoder = Some(Encoder::start(parameters, (frame.width(), frame.height()))
                            .chain_err(|| "could not start the encoder")?);
    }

    let mut encoder = encoder.as_mut().unwrap();

    ensure!((frame.width(), frame.height()) == (encoder.width(), encoder.height()),
            "resolution changes are not supported");

    // Encode the frame.
    // TODO: correct argument.
    encoder.take(frame, 1)
           .chain_err(|| "could not encode the frame")?;

    Ok(())
}

pub fn initialize(_: &Engine) {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        let (tx, rx) = channel::<VideoBuffer>();
        let (tx2, rx2) = channel::<AudioBuffer>();
        let (tx3, rx3) = channel::<String>();
        let (tx4, rx4) = channel::<CaptureThreadEvent>();

        *VIDEO_BUF_RECEIVER.lock().unwrap() = Some(rx);
        *AUDIO_BUF_RECEIVER.lock().unwrap() = Some(rx2);
        *MESSAGE_RECEIVER.lock().unwrap() = Some(rx3);
        *SEND_TO_CAPTURE_THREAD.lock().unwrap() = Some(tx4);

        thread::spawn(move || capture_thread(&tx, &tx2, &tx3, &rx4));
    });
}

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

pub fn get_audio_buffer(_: &Engine) -> AudioBuffer {
    AUDIO_BUF_RECEIVER.lock()
                      .unwrap()
                      .as_ref()
                      .unwrap()
                      .recv()
                      .unwrap()
}

pub fn get_message(_: &Engine) -> Option<String> {
    match MESSAGE_RECEIVER.lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .try_recv() {
        Ok(msg) => Some(msg),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => unreachable!(),
    }
}

pub fn capture(_: &Engine, buf: VideoBuffer, frametime: f64) {
    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::VideoFrame((buf, frametime)))
                          .unwrap();
}

pub fn capture_audio(_: &Engine, buf: AudioBuffer) {
    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::AudioFrame(buf))
                          .unwrap();
}

pub fn is_capturing() -> bool {
    *CAPTURING.read().unwrap()
}

pub fn get_capture_parameters(engine: &Engine) -> &CaptureParameters {
    engine.data().capture_parameters.as_ref().unwrap()
}

pub fn stop(engine: &Engine) {
    hw::capture_remaining_sound(engine);

    *CAPTURING.write().unwrap() = false;

    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::CaptureStop)
                          .unwrap();

    GAME_THREAD_PROFILER.with(|p| if let Some(p) = p.borrow_mut().take() {
        if let Ok(data) = p.get_data() {
            let mut buf = format!("Captured {} frames. Game thread overhead: {:.3} msec:\n",
                                  data.lap_count,
                                  data.average_lap_time);

            for &(section, time) in &data.average_section_times {
                buf.push_str(&format!("- {:.3} msec: {}\n", time, section));
            }

            engine.con_print(&buf);
        }
    });
    AUDIO_PROFILER.with(|p| if let Some(p) = p.borrow_mut().take() {
                            if let Ok(data) = p.get_data() {
                                let mut buf = format!("Audio overhead: {:.3} msec:\n",
                                                      data.average_lap_time);

                                for &(section, time) in &data.average_section_times {
                                    buf.push_str(&format!("- {:.3} msec: {}\n", time, section));
                                }

                                engine.con_print(&buf);
                            }
                        });
}

/// Parses the given string and returns a time base.
///
/// The string can be in one of the two formats:
/// - `<i32 FPS>` - treated as an integer FPS value,
/// - `<i32 a> <i32 b>` - treated as a fractional `a/b` FPS value.
fn parse_fps(string: &str) -> Option<Rational> {
    if let Ok(fps) = string.parse() {
        return Some((1, fps).into());
    }

    let mut split = string.splitn(2, ' ');
    if let Some(den) = split.next().and_then(|s| s.parse().ok()) {
        if let Some(num) = split.next().and_then(|s| s.parse().ok()) {
            return Some((num, den).into());
        }
    }

    None
}

macro_rules! to_string {
    ($engine:expr, $cvar:expr) => (
        $cvar.to_string($engine).chain_err(|| concat!("invalid ", stringify!($cvar)))?
    )
}

macro_rules! parse {
    ($engine:expr, $cvar:expr) => (
        $cvar.parse($engine).chain_err(|| concat!("invalid ", stringify!($cvar)))?
    )
}

/// Parses the CVar values into `EncoderParameters`.
fn parse_encoder_parameters(engine: &mut Engine) -> Result<EncoderParameters> {

    Ok(EncoderParameters {
           audio_bitrate: parse!(engine, cap_audio_bitrate),
           video_bitrate: parse!(engine, cap_video_bitrate),
           crf: to_string!(engine, cap_crf),
           filename: to_string!(engine, cap_filename),
           muxer_settings: to_string!(engine, cap_muxer_settings),
           preset: to_string!(engine, cap_x264_preset),
           time_base: parse_fps(&to_string!(engine, cap_fps))
               .ok_or("invalid cap_fps")?,
           audio_encoder_settings: to_string!(engine, cap_audio_encoder_settings),
           video_encoder_settings: to_string!(engine, cap_video_encoder_settings),
           vpx_cpu_usage: to_string!(engine, cap_vpx_cpu_usage),
           vpx_threads: to_string!(engine, cap_vpx_threads),
       })
}

/// Parses the CVar values into `CaptureParameters`.
fn parse_capture_parameters(engine: &mut Engine) -> Result<CaptureParameters> {
    Ok(CaptureParameters {
           sound_extra: parse!(engine, cap_sound_extra),
           time_base: parse_fps(&to_string!(engine, cap_fps))
               .ok_or("invalid cap_fps")?,
           volume: parse!(engine, cap_volume),
       })
}

command!(cap_start, |mut engine| {
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

    *CAPTURING.write().unwrap() = true;

    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::CaptureStart(parameters))
                          .unwrap();

    GAME_THREAD_PROFILER.with(|p| *p.borrow_mut() = Some(Profiler::new()));
    AUDIO_PROFILER.with(|p| *p.borrow_mut() = Some(Profiler::new()));

    hw::reset_sound_capture_remainder(&engine);
});

command!(cap_stop, |engine| { stop(&engine); });

// Encoder parameters.
cvar!(cap_video_bitrate, "0");
cvar!(cap_audio_bitrate, "320000");
cvar!(cap_crf, "15");
cvar!(cap_filename, "capture.mp4");
cvar!(cap_fps, "60");
cvar!(cap_muxer_settings, "");
cvar!(cap_audio_encoder_settings, "");
cvar!(cap_video_encoder_settings, "");
cvar!(cap_vpx_cpu_usage, "5");
cvar!(cap_vpx_threads, "8");
cvar!(cap_x264_preset, "veryfast");

// Capture parameters.
cvar!(cap_sound_extra, "0");
cvar!(cap_volume, "0.4");
