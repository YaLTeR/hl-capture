use fine_grained::Stopwatch;
use std::collections::HashMap;

use errors::*;

/// A profiler that gathers average run times code sections.
pub struct Profiler {
    main_watch: Stopwatch,
    watches: HashMap<&'static str, Stopwatch>,
    current_section: Option<&'static str>,
}

/// Profiling data collected by the `Profiler`.
pub struct ProfilingData {
    average_total_time: f64,
    average_section_times: HashMap<&'static str, f64>,
}

impl Profiler {
    /// Returns a new profiler.
    pub fn new() -> Self {
        Self {
            main_watch: Stopwatch::new(),
            watches: HashMap::with_capacity(10),
            current_section: None,
        }
    }

    /// Starts the timer corresponding to the given section.
    ///
    /// The currently running timer, if any, is stopped.
    pub fn start_section(&mut self, name: &'static str) {
        if self.current_section.is_some() {
            self.stop_current_section().unwrap();
        } else {
            self.main_watch.start();
        }

        self.watches
            .entry(name)
            .or_insert_with(Stopwatch::new)
            .start();

        self.current_section = Some(name);
    }

    /// Stops the currently running timer.
    fn stop_current_section(&mut self) -> Result<()> {
        ensure!(self.current_section.is_some(),
                "no stopwatches are currently running");

        let mut stopwatch =
            self.watches
                .get_mut(self.current_section.unwrap())
                .expect("current_section was set to an invalid value");
        stopwatch.lap();
        stopwatch.stop();

        Ok(())
    }

    /// Checks if lap counters for all watches match.
    fn check_lap_counters(&self) -> bool {
        let lap_count = self.main_watch.number_of_laps();

        for watch in self.watches.values() {
            if watch.number_of_laps() != lap_count {
                return false;
            }
        }

        true
    }

    /// Stops timing the current run and increases the lap counter.
    pub fn stop(&mut self) -> Result<()> {
        self.stop_current_section()?;

        self.main_watch.lap();
        self.main_watch.stop();

        Ok(())
    }

    /// Returns the collected data.
    ///
    /// The collected data includes the total average time, as well as average times for
    /// individual sections.
    pub fn get_data(&self) -> Result<ProfilingData> {
        debug_assert!(self.check_lap_counters(), "lap counters do not match");

        let lap_count = self.main_watch.number_of_laps() as f64;
        ensure!(lap_count > 0f64, "no data has been collected");

        Ok(ProfilingData {
               average_total_time: self.main_watch.total_time() as f64 / lap_count,
               average_section_times: self.watches
                                          .iter()
                                          .map(|(&section, watch)| {
                                                   (section, watch.total_time() as f64 / lap_count)
                                               })
                                          .collect(),
           })
    }
}
