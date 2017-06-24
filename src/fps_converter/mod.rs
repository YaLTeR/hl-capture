use engine::Engine;
use hooks::hw;

mod simple;
pub use self::simple::SimpleConverter;

pub trait FPSConverter {
    /// Updates the FPS converter state. The converter may capture one frame using the provided
    /// closure.
    fn time_passed<F>(&mut self, engine: &Engine, frametime: f64, capture: F)
    where
        F: FnOnce(&Engine) -> hw::FrameCapture;
}

pub enum FPSConverters {
    Simple(SimpleConverter),
}
