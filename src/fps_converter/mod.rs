use crate::engine::Engine;
use crate::hooks::hw;

mod sampling;
mod simple;
pub use self::sampling::SamplingConverter;
pub use self::simple::SimpleConverter;

pub trait FPSConverter {
    /// Updates the FPS converter state. The converter may capture one frame using the provided
    /// closure.
    fn time_passed<F>(&mut self, engine: &mut Engine, frametime: f64, capture: F)
        where F: FnOnce(&mut Engine) -> hw::FrameCapture;
}

pub enum FPSConverters {
    Simple(SimpleConverter),
    Sampling(SamplingConverter),
}
