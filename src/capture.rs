use error_chain::ChainedError;
use ffmpeg;
use std::ptr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, RwLock, Once, ONCE_INIT};
use std::thread;

use encode;
use errors::*;
use Frame;

lazy_static! {
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);

    static ref ENCODER: Mutex<Option<encode::Encoder>> = Mutex::new(None);

    /// Receives frames to write pixels to.
    static ref FRAME_RECEIVER: Mutex<Option<Receiver<Buffer>>> = Mutex::new(None);

    /// Sends frames to encode.
    static ref FRAME_SENDER: Mutex<Option<Sender<Buffer>>> = Mutex::new(None);
}

pub struct Buffer {
    pub data: Vec<u8>,
    width: u32,
    height: u32,
}

fn capture_thread(frame_sender: Sender<Buffer>, frame_receiver: Receiver<Buffer>) {
    let mut frame = ffmpeg::frame::Video::empty();
    let mut buf = Buffer {
        data: Vec::new(),
        width: 0,
        height: 0,
    };

    loop {
        frame_sender.send(buf).unwrap();
        buf = frame_receiver.recv().unwrap();

        // Make sure frame is of correct size.
        if buf.width != frame.width() || buf.height != frame.height() {
            frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, buf.width, buf.height);
        }

        // Copy the pixel data into the frame.
        {
            let linesize = unsafe { ((*frame.as_ptr()).linesize[0]) as u32 };
            let mut data = frame.data_mut(0);

            for y in 0..buf.height {
                unsafe {
                    ptr::copy_nonoverlapping(buf.data.as_ptr().offset((y * buf.width * 3) as isize),
                                             data.as_mut_ptr().offset(((buf.height - y - 1) * linesize) as isize),
                                             (buf.width * 3) as usize);
                }
            }
        }

        let mut encoder = ENCODER.lock().unwrap();

        if encoder.as_ref().map_or(true, |enc| {
                enc.width() != frame.width() || enc.height() != frame.height()
            }) {
            *encoder = start_encoder((frame.width(), frame.height()));
        }

        if encoder.is_none() {
            continue;
        }

        if let Err(ref e) = encoder.as_mut().unwrap().encode(&frame)
            .chain_err(|| "could not encode a frame") {
            println!("{}", e.display());
            // TODO: while we were getting here the main thread potentially
            // got into the CAPTURING if the second time already.
            *CAPTURING.write().unwrap() = false;
        };

        // If the capture was stopped, drop the encoder.
        if !*CAPTURING.read().unwrap() {
            *encoder = None;
        }
    }
}

fn start_encoder((width, height): (u32, u32)) -> Option<encode::Encoder> {
    match encode::Encoder::start("/home/yalter/test.mp4",
                                 (width, height),
                                 (1, 60).into()) {
        Ok(enc) => Some(enc),
        Err(ref e) => {
            println!("could not create the encoder: {}", e.display());
            // TODO: while we were getting here the main thread potentially
            // got into the CAPTURING if the second time already. This means
            // that our error message will be printed twice.
            *CAPTURING.write().unwrap() = false;
            None
        }
    }
}

pub fn initialize() {
    static INIT: Once = ONCE_INIT;
    INIT.call_once(|| {
        let (tx, rx) = channel::<Buffer>();
        let (tx2, rx2) = channel::<Buffer>();

        *FRAME_RECEIVER.lock().unwrap() = Some(rx);
        *FRAME_SENDER.lock().unwrap() = Some(tx2);

        thread::spawn(move || capture_thread(tx, rx2));
    });
}

pub fn get_buffer((width, height): (u32, u32)) -> Buffer {
    let mut buf = FRAME_RECEIVER.lock().unwrap().as_ref().unwrap().recv().unwrap();

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
    FRAME_SENDER.lock().unwrap().as_ref().unwrap().send(buf).unwrap();
}

pub fn is_capturing() -> bool {
    *CAPTURING.read().unwrap()
}

command!(cap_start, |_engine| {
    *CAPTURING.write().unwrap() = true;
});

command!(cap_stop, |_engine| {
    *CAPTURING.write().unwrap() = false;
});
