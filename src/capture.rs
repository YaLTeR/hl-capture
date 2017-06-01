use error_chain::ChainedError;
use ffmpeg;
use std::ptr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, RwLock, Once, ONCE_INIT};
use std::thread;

use encode;
use errors::*;

lazy_static! {
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);

    static ref ENCODER: Mutex<Option<encode::Encoder>> = Mutex::new(None);

    /// Receives buffers to write pixels to.
    static ref BUF_RECEIVER: Mutex<Option<Receiver<Buffer>>> = Mutex::new(None);

    /// Sends events and frames to encode to the capture thread.
    static ref SEND_TO_CAPTURE_THREAD: Mutex<Option<Sender<CaptureThreadEvent>>> = Mutex::new(None);
}

enum CaptureThreadEvent {
    CaptureStart,
    CaptureStop,
    Frame(Buffer),
}

pub struct Buffer {
    pub data: Vec<u8>,
    width: u32,
    height: u32,
}

fn capture_thread(buf_sender: Sender<Buffer>, event_receiver: Receiver<CaptureThreadEvent>) {
    // Send the buffer to the game thread right away.
    buf_sender.send(Buffer {
                        data: Vec::new(),
                        width: 0,
                        height: 0,
                    })
              .unwrap();

    // This is our frame which will only be reallocated on resolution changes.
    let mut frame = ffmpeg::frame::Video::empty();

    // This is set to true on encoding error or cap_stop and reset on cap_start.
    // When this is true, ignore any received frames.
    let mut drop_frames = true;

    // Event loop for the capture thread.
    loop {
        match event_receiver.recv().unwrap() {
            CaptureThreadEvent::CaptureStart => {
                drop_frames = false;
            }

            CaptureThreadEvent::CaptureStop => {
                *ENCODER.lock().unwrap() = None;
                drop_frames = true;
            }

            CaptureThreadEvent::Frame(buffer) => {
                if drop_frames {
                    continue;
                }

                if let Err(ref e) = encode(buffer, &buf_sender, &mut frame) {
                    *CAPTURING.write().unwrap() = false;
                    drop_frames = true;
                    println!("Encoding error: {}", e.display());
                }
            }
        }
    }
}

fn start_encoder((width, height): (u32, u32)) -> Result<encode::Encoder> {
    encode::Encoder::start("/home/yalter/test.mp4", (width, height), (1, 60).into())
}

fn encode(buf: Buffer,
          buf_sender: &Sender<Buffer>,
          frame: &mut ffmpeg::frame::Video)
          -> Result<()> {
    // Make sure frame is of correct size.
    if buf.width != frame.width() || buf.height != frame.height() {
        *frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, buf.width, buf.height);
    }

    // Copy the pixel data into the frame.
    {
        let linesize = unsafe { ((*frame.as_ptr()).linesize[0]) as u32 };
        let mut data = frame.data_mut(0);

        for y in 0..buf.height {
            unsafe {
                ptr::copy_nonoverlapping(buf.data.as_ptr().offset((y * buf.width * 3) as isize),
                                         data.as_mut_ptr()
                                             .offset(((buf.height - y - 1) * linesize) as
                                                     isize),
                                         (buf.width * 3) as usize);
            }
        }
    }

    // We're done with buf, now it can receive the next pack of pixels.
    buf_sender.send(buf).unwrap();

    // Let's encode the frame we just received.
    let mut encoder = ENCODER.lock().unwrap();

    // If the encoder wasn't initialized or if the frame size changed, initialize it.
    if encoder.as_ref()
              .map_or(true,
                      |enc| enc.width() != frame.width() || enc.height() != frame.height()) {
        *encoder = Some(start_encoder((frame.width(), frame.height()))
                            .chain_err(|| "could not start the video encoder")?);
    }

    // Encode the frame.
    encoder.as_mut().unwrap().encode(&frame)
        .chain_err(|| "could not encode the frame")?;

    Ok(())
}

pub fn initialize() {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        let (tx, rx) = channel::<Buffer>();
        let (tx2, rx2) = channel::<CaptureThreadEvent>();

        *BUF_RECEIVER.lock().unwrap() = Some(rx);
        *SEND_TO_CAPTURE_THREAD.lock().unwrap() = Some(tx2);

        thread::spawn(move || capture_thread(tx, rx2));
    });
}

pub fn get_buffer((width, height): (u32, u32)) -> Buffer {
    let mut buf = BUF_RECEIVER.lock().unwrap()
        .as_ref().unwrap()
        .recv().unwrap();

    if buf.width != width || buf.height != height {
        println!("Changing resolution from {:?} to {:?}.",
                 (buf.width, buf.height),
                 (width, height));

        buf.data.resize((width * height * 3) as usize, 0);
        buf.width = width;
        buf.height = height;
    }

    buf
}

pub fn capture(buf: Buffer) {
    SEND_TO_CAPTURE_THREAD.lock().unwrap()
        .as_ref().unwrap()
        .send(CaptureThreadEvent::Frame(buf)).unwrap();
}

pub fn is_capturing() -> bool {
    *CAPTURING.read().unwrap()
}

command!(cap_start, |_engine| {
    *CAPTURING.write().unwrap() = true;

    SEND_TO_CAPTURE_THREAD.lock().unwrap()
        .as_ref().unwrap()
        .send(CaptureThreadEvent::CaptureStart).unwrap();
});

command!(cap_stop, |_engine| {
    *CAPTURING.write().unwrap() = false;

    SEND_TO_CAPTURE_THREAD.lock().unwrap()
        .as_ref().unwrap()
        .send(CaptureThreadEvent::CaptureStop).unwrap();
});
