use crate::engine::MainThreadMarker;
use crate::hooks::hw;

mod sampling;
mod simple;
pub use self::sampling::SamplingConverter;
pub use self::simple::SimpleConverter;

pub trait FPSConverter {
    /// Updates the FPS converter state. The converter may capture one frame using the provided
    /// closure.
    fn time_passed<F>(&mut self, marker: MainThreadMarker, frametime: f64, capture: F)
        where F: FnOnce(MainThreadMarker) -> hw::FrameCapture;
}

pub enum FPSConverters {
    Simple(SimpleConverter),
    Sampling(SamplingConverter),
}
