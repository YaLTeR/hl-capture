use error_chain::ChainedError;
use ffmpeg;
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
    static ref FRAME_RECEIVER: Mutex<Option<Receiver<Frame>>> = Mutex::new(None);

    /// Sends frames to encode.
    static ref FRAME_SENDER: Mutex<Option<Sender<Frame>>> = Mutex::new(None);
}

fn capture_thread(frame_sender: Sender<Frame>, frame_receiver: Receiver<Frame>) {
    let mut frame = ffmpeg::frame::Video::empty();

    loop {
        frame_sender.send(frame).unwrap();
        frame = frame_receiver.recv().unwrap();

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
        let (tx, rx) = channel::<Frame>();
        let (tx2, rx2) = channel::<Frame>();

        *FRAME_RECEIVER.lock().unwrap() = Some(rx);
        *FRAME_SENDER.lock().unwrap() = Some(tx2);

        thread::spawn(move || capture_thread(tx, rx2));
    });
}

pub fn get_buffer((width, height): (u32, u32)) -> Frame {
    let mut frame = FRAME_RECEIVER.lock().unwrap().as_ref().unwrap().recv().unwrap();

    if frame.width() != width || frame.height() != height {
        println!("Changing resolution from {:?} to {:?}.",
                 (frame.width(), frame.height()),
                 (width, height));

        frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, width, height);
    }

    frame
}

pub fn capture(frame: Frame) {
    FRAME_SENDER.lock().unwrap().as_ref().unwrap().send(frame).unwrap();
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
