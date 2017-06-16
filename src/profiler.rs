use fine_grained::Stopwatch;
use std::collections::HashMap;

use errors::*;

/// A profiler that gathers average run times code sections.
pub struct Profiler {
    main_watch: Stopwatch,
    watches: HashMap<&'static str, (usize, Stopwatch)>,
    current_section: Option<&'static str>,
}

/// Profiling data collected by the `Profiler`.
pub struct ProfilingData {
    /// Number of laps.
    pub lap_count: usize,

    /// Average lap time in milliseconds.
    pub average_lap_time: f64,

    /// Average time of each section in milliseconds.
    /// The vector is sorted according to the section order.
    pub average_section_times: Vec<(&'static str, f64)>,
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
            self.stop_current_section(false).unwrap();
        } else {
            self.main_watch.start();
        }

        let len = self.watches.len();
        let &mut (_, ref mut stopwatch) =
            self.watches.entry(name).or_insert((len, Stopwatch::new()));
        stopwatch.start();

        self.current_section = Some(name);
    }

    /// Stops the currently running timer.
    ///
    /// If `cancel` is set to `true`, the measurement is discarded.
    fn stop_current_section(&mut self, cancel: bool) -> Result<()> {
        ensure!(
            self.current_section.is_some(),
            "no stopwatches are currently running"
        );

        let &mut (_, ref mut stopwatch) =
            self.watches.get_mut(self.current_section.unwrap()).expect(
                "current_section was set to an invalid value",
            );

        if !cancel {
            stopwatch.lap();
        }

        stopwatch.stop();

        self.current_section = None;

        Ok(())
    }

    /// Checks if lap counters for all watches match.
    fn check_lap_counters(&self) -> bool {
        let lap_count = self.main_watch.number_of_laps();

        for &(_, ref watch) in self.watches.values() {
            if watch.number_of_laps() != lap_count {
                return false;
            }
        }

        true
    }

    /// Stops timing the current run and increases the lap counter.
    ///
    /// If `cancel` is set to `true`, the measurement is discarded.
    pub fn stop(&mut self, cancel: bool) -> Result<()> {
        self.stop_current_section(cancel)?;

        if !cancel {
            self.main_watch.lap();
        }

        self.main_watch.stop();

        Ok(())
    }

    /// Returns the collected data.
    ///
    /// The collected data includes the total average time, as well as average times for
    /// individual sections.
    pub fn get_data(&self) -> Result<ProfilingData> {
        debug_assert!(self.check_lap_counters(), "lap counters do not match");

        let lap_count = self.main_watch.number_of_laps();
        ensure!(lap_count > 0, "no data has been collected");

        let denom = (lap_count * 1000) as u64;

        let mut sections = self.watches.iter().collect::<Vec<_>>();
        sections.sort_by_key(|&(_, &(pos, _))| pos);

        Ok(ProfilingData {
            lap_count,
            average_lap_time: (self.main_watch.total_time() / denom) as f64 / 1000f64,
            average_section_times: sections.iter()
                                           .map(|&(&section, &(_, ref watch))| {
                (section, (watch.total_time() / denom) as f64 / 1000f64)
            })
                                           .collect(),
        })
    }
}
