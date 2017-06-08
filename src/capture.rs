use error_chain::ChainedError;
use ffmpeg::{Rational, format};
use ffmpeg::frame::Video as VideoFrame;
use fine_grained::Stopwatch;
use std::cell::RefCell;
use std::ptr;
use std::sync::{Mutex, ONCE_INIT, Once, RwLock};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;

use encode::{Encoder, EncoderParameters};
use errors::*;

lazy_static! {
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);

    // TODO: this is bad, refactor this.
    static ref TIME_BASE: RwLock<Option<Rational>> = RwLock::new(None);

    static ref ENCODER: Mutex<Option<Encoder>> = Mutex::new(None);

    /// Receives buffers to write pixels to.
    static ref BUF_RECEIVER: Mutex<Option<Receiver<Buffer>>> = Mutex::new(None);

    /// Receives errors.
    static ref ERROR_RECEIVER: Mutex<Option<Receiver<Error>>> = Mutex::new(None);

    /// Sends events and frames to encode to the capture thread.
    static ref SEND_TO_CAPTURE_THREAD: Mutex<Option<Sender<CaptureThreadEvent>>> = Mutex::new(None);
}

thread_local! {
    static STOPWATCH: RefCell<Option<Stopwatch>> = RefCell::new(None);
}

enum CaptureThreadEvent {
    CaptureStart(EncoderParameters),
    CaptureStop,
    Frame((Buffer, f64)),
}

pub struct Buffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

impl Buffer {
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

            self.data.resize((width * height * 3) as usize, 0);
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
            *frame = VideoFrame::new(format::Pixel::RGB24, self.width, self.height);
        }

        // Copy the pixel data into the frame.
        let linesize = unsafe { ((*frame.as_ptr()).linesize[0]) as u32 };
        let mut data = frame.data_mut(0);

        for y in 0..self.height {
            unsafe {
                ptr::copy_nonoverlapping(self.data.as_ptr().offset((y * self.width * 3) as isize),
                                         data.as_mut_ptr()
                                             .offset(((self.height - y - 1) * linesize) as
                                                     isize),
                                         (self.width * 3) as usize);
            }
        }
    }
}

fn capture_thread(buf_sender: &Sender<Buffer>, error_sender: &Sender<Error>, event_receiver: &Receiver<CaptureThreadEvent>) {
    // Send the buffer to the game thread right away.
    buf_sender.send(Buffer::new()).unwrap();

    // This is our frame which will only be reallocated on resolution changes.
    let mut frame = VideoFrame::empty();

    // This is set to true on encoding error or cap_stop and reset on cap_start.
    // When this is true, ignore any received frames.
    let mut drop_frames = true;

    // Encoding parameters, set on CaptureStart.
    let mut parameters = None;

    // Event loop for the capture thread.
    loop {
        match event_receiver.recv().unwrap() {
            CaptureThreadEvent::CaptureStart(params) => {
                drop_frames = false;
                parameters = Some(params);
            }

            CaptureThreadEvent::CaptureStop => {
                if let Some(mut encoder) = (*ENCODER.lock().unwrap()).take() {
                    if let Err(e) = encoder.finish() {
                        error_sender.send(e).unwrap();
                    }

                    // The encoder is dropped here.
                }

                drop_frames = true;
            }

            CaptureThreadEvent::Frame((buffer, frametime)) => {
                if drop_frames {
                    buf_sender.send(buffer).unwrap();
                    continue;
                }

                if let Err(e) = encode(buffer,
                                       frametime,
                                       &buf_sender,
                                       parameters.as_ref().unwrap(),
                                       &mut frame) {
                    *CAPTURING.write().unwrap() = false;
                    drop_frames = true;

                    error_sender.send(e).unwrap();
                }
            }
        }
    }
}

fn start_encoder(parameters: &EncoderParameters,
                 (width, height): (u32, u32))
                 -> Result<Encoder> {
    Encoder::start(&parameters, (width, height))
}

fn encode(buf: Buffer,
          frametime: f64,
          buf_sender: &Sender<Buffer>,
          parameters: &EncoderParameters,
          frame: &mut VideoFrame)
          -> Result<()> {
    buf.copy_to_frame(frame);

    // We're done with buf, now it can receive the next pack of pixels.
    // IMPORTANT: if there is any error handling in this function before this line, make sure to
    // handle sending the buffer back in the error handling appropriately, as otherwise the game
    // will freeze.
    // TODO: restructure the code to get rid of the above condition.
    buf_sender.send(buf).unwrap();

    // Let's encode the frame we just received.
    let mut encoder = ENCODER.lock().unwrap();

    // If the encoder wasn't initialized or if the frame size changed, initialize it.
    if encoder.as_ref()
              .map_or(true,
                      |enc| enc.width() != frame.width() || enc.height() != frame.height()) {
        *encoder = Some(start_encoder(&parameters,
                                      (frame.width(), frame.height()))
                            .chain_err(|| "could not start the video encoder")?);
    }

    // Encode the frame.
    encoder.as_mut()
           .unwrap()
           .take(frame, frametime)
           .chain_err(|| "could not encode the frame")?;

    Ok(())
}

pub fn initialize() {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        let (tx, rx) = channel::<Buffer>();
        let (tx2, rx2) = channel::<Error>();
        let (tx3, rx3) = channel::<CaptureThreadEvent>();

        *BUF_RECEIVER.lock().unwrap() = Some(rx);
        *ERROR_RECEIVER.lock().unwrap() = Some(rx2);
        *SEND_TO_CAPTURE_THREAD.lock().unwrap() = Some(tx3);

        thread::spawn(move || capture_thread(&tx, &tx2, &rx3));
    });
}

pub fn get_buffer((width, height): (u32, u32)) -> Buffer {
    let mut buf = BUF_RECEIVER.lock()
                              .unwrap()
                              .as_ref()
                              .unwrap()
                              .recv()
                              .unwrap();

    buf.set_resolution(width, height);

    buf
}

pub fn get_error() -> Option<Error> {
    match ERROR_RECEIVER.lock().unwrap().as_ref().unwrap().try_recv() {
        Ok(e) => Some(e),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => unreachable!(),
    }
}

pub fn capture(buf: Buffer, frametime: f64) {
    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::Frame((buf, frametime)))
                          .unwrap();
}

pub fn is_capturing() -> bool {
    *CAPTURING.read().unwrap()
}

pub fn capture_block_start() {
    STOPWATCH.with(|sw| if let Some(sw) = sw.borrow_mut().as_mut() {
                       sw.start();
                   });
}

pub fn capture_block_end() {
    STOPWATCH.with(|sw| if let Some(sw) = sw.borrow_mut().as_mut() {
                       sw.lap();
                       sw.stop();
                   });
}

pub fn get_frametime() -> Option<f64> {
    TIME_BASE.read().unwrap().map(|x| x.into())
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

command!(cap_start, |mut engine| {
    let mut parameters = EncoderParameters {
        bitrate: 0,
        crf: String::new(),
        filename: String::new(),
        muxer_settings: String::new(),
        preset: String::new(),
        time_base: Rational::new(1, 1),
        video_encoder_settings: String::new(),
        vpx_cpu_usage: String::new(),
        vpx_threads: String::new(),
    };

    match cap_filename.get(&engine)
                      .to_string(&mut engine)
                      .chain_err(|| "invalid cap_filename") {
        Ok(filename) => parameters.filename = filename,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    if let Some(time_base) = cap_fps.get(&engine)
                                    .to_string(&mut engine)
                                    .ok()
                                    .and_then(|s| parse_fps(&s)) {
        parameters.time_base = time_base;
    } else {
        engine.con_print("Invalid cap_fps.\n");
        return;
    }

    match cap_crf.get(&engine)
                 .parse(&mut engine)
                 .chain_err(|| "invalid cap_crf") {
        Ok(crf) => parameters.crf = crf,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    match cap_bitrate.get(&engine)
                     .parse(&mut engine)
                     .chain_err(|| "invalid cap_bitrate") {
        Ok(bitrate) => parameters.bitrate = bitrate,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    match cap_muxer_settings.get(&engine)
                            .parse(&mut engine)
                            .chain_err(|| "invalid cap_muxer_settings") {
        Ok(muxer_settings) => parameters.muxer_settings = muxer_settings,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    match cap_x264_preset.get(&engine)
                         .parse(&mut engine)
                         .chain_err(|| "invalid cap_x264_preset") {
        Ok(preset) => parameters.preset = preset,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    match cap_video_encoder_settings.get(&engine)
                                    .parse(&mut engine)
                                    .chain_err(|| "invalid cap_video_encoder_settings") {
        Ok(video_encoder_settings) => parameters.video_encoder_settings = video_encoder_settings,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    match cap_vpx_cpu_usage.get(&engine)
                         .parse(&mut engine)
                         .chain_err(|| "invalid cap_vpx_cpu_usage") {
        Ok(cpu_usage) => parameters.vpx_cpu_usage = cpu_usage,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    match cap_vpx_threads.get(&engine)
                         .parse(&mut engine)
                         .chain_err(|| "invalid cap_vpx_threads") {
        Ok(threads) => parameters.vpx_threads = threads,
        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
            return;
        }
    };

    *CAPTURING.write().unwrap() = true;
    *TIME_BASE.write().unwrap() = Some(parameters.time_base);

    SEND_TO_CAPTURE_THREAD.lock()
                          .unwrap()
                          .as_ref()
                          .unwrap()
                          .send(CaptureThreadEvent::CaptureStart(parameters))
                          .unwrap();

    STOPWATCH.with(|sw| *sw.borrow_mut() = Some(Stopwatch::new()));
});

command!(cap_stop, |engine| {
    *CAPTURING.write().unwrap() = false;

    SEND_TO_CAPTURE_THREAD.lock().unwrap()
        .as_ref().unwrap()
        .send(CaptureThreadEvent::CaptureStop).unwrap();

    STOPWATCH.with(|sw| if let Some(sw) = sw.borrow_mut().take() {
        let frames = sw.number_of_laps();

        if frames > 0 {
            engine.con_print(&format!("Captured {} frames in {} seconds (~{} msec of overhead per frame)\n",
                                      frames,
                                      sw.total_time() as f64 / 1_000_000_000f64,
                                      (sw.total_time() / frames as u64) as f64 / 1_000_000f64));
        }
    });
});

cvar!(cap_bitrate, "0");
cvar!(cap_crf, "15");
cvar!(cap_filename, "capture.mp4");
cvar!(cap_fps, "60");
cvar!(cap_muxer_settings, "");
cvar!(cap_video_encoder_settings, "");
cvar!(cap_vpx_cpu_usage, "5");
cvar!(cap_vpx_threads, "8");
cvar!(cap_x264_preset, "veryfast");
