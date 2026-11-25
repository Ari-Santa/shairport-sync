//! The receiver's `Host` in a Qt program: timers, clocks and the PipeWire
//! tools, each reporting back on the Qt thread.

use std::io::Read;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cxx_qt::CxxQtThread;
use cxx_qt_lib::{QString, QUrl};

use crate::object::qobject::ReceiverObject;
use crate::receiver::{Host, Receiver, Timer};
use crate::shim;

const POLL_UNIT_SOON: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Schedule {
    /// When each running timer fires next, and the start it belongs to.
    deadlines: HashMap<Timer, (Instant, u64)>,
    /// The start each timer that may still report belongs to.
    live: HashMap<Timer, u64>,
    /// Repeating timers that have fired and not reported yet.
    pending: HashSet<Timer>,
    starts: u64,
    /// When each `poll_unit_soon` is due.
    soon: Vec<Instant>,
    closed: bool,
}

type Shared = Arc<(Mutex<Schedule>, Condvar)>;

/// Run `f` on the receiver, on the Qt thread; false if the object has gone.
fn queue(qt_thread: &CxxQtThread<ReceiverObject>, f: impl FnOnce(&mut Receiver) + Send + 'static) -> bool {
    qt_thread.queue(move |object: Pin<&mut ReceiverObject>| object.with_receiver(f)).is_ok()
}

fn run_timers(shared: Shared, qt_thread: CxxQtThread<ReceiverObject>) {
    let (schedule, wake) = &*shared;
    let mut schedule = schedule.lock().unwrap();
    while !schedule.closed {
        let now = Instant::now();
        let mut fired = Vec::new();
        for (&timer, deadline) in schedule.deadlines.iter_mut() {
            if deadline.0 <= now {
                fired.push((timer, deadline.1));
                deadline.0 = now + Duration::from_millis(timer.interval_ms());
            }
        }
        // A repeating timer that fires again before it has reported fires once,
        // as Qt's do. A single-shot one fires once per start anyway, and a fire
        // of an earlier start is dropped when it is delivered.
        fired.retain(|(timer, _)| {
            if timer.single_shot() {
                schedule.deadlines.remove(timer);
                return true;
            }
            schedule.pending.insert(*timer)
        });
        let soon = schedule.soon.len();
        schedule.soon.retain(|due| *due > now);
        let soon = soon - schedule.soon.len();

        if !fired.is_empty() || soon > 0 {
            drop(schedule);
            let mut alive = true;
            for (timer, start) in fired {
                let shared = shared.clone();
                alive &= queue(&qt_thread, move |receiver| {
                    let current = {
                        let mut schedule = shared.0.lock().unwrap();
                        schedule.pending.remove(&timer);
                        let current = schedule.live.get(&timer) == Some(&start);
                        if current && timer.single_shot() {
                            schedule.live.remove(&timer);
                        }
                        current
                    };
                    // A timer stopped or started again meanwhile doesn't report.
                    if current {
                        receiver.on_timer(timer);
                    }
                });
            }
            for _ in 0..soon {
                alive &= queue(&qt_thread, Receiver::on_poll_unit_soon);
            }
            if !alive {
                return;
            }
            schedule = shared.0.lock().unwrap();
            continue;
        }

        let next = schedule.deadlines.values().map(|deadline| deadline.0).chain(schedule.soon.iter().copied()).min();
        schedule = match next {
            Some(next) => wake.wait_timeout(schedule, next.saturating_duration_since(now)).unwrap().0,
            None => wake.wait(schedule).unwrap(),
        };
    }
}

pub struct QtHost {
    qt_thread: CxxQtThread<ReceiverObject>,
    timers: Shared,
    pw_dump_running: Arc<AtomicBool>,
    /// The pw-dump that hasn't finished yet, to kill when the app quits.
    pw_dump: Arc<Mutex<Option<Child>>>,
    started: Instant,
}

impl QtHost {
    pub fn new(qt_thread: CxxQtThread<ReceiverObject>) -> Self {
        let timers = Shared::default();
        let (shared, thread) = (timers.clone(), qt_thread.clone());
        std::thread::spawn(move || run_timers(shared, thread));
        QtHost { qt_thread, timers, pw_dump_running: Arc::default(), pw_dump: Arc::default(), started: Instant::now() }
    }

    fn schedule(&self, change: impl FnOnce(&mut Schedule)) {
        change(&mut self.timers.0.lock().unwrap());
        self.timers.1.notify_all();
    }
}

impl Drop for QtHost {
    fn drop(&mut self) {
        self.schedule(|schedule| schedule.closed = true);
        // As QProcess did: a pw-dump that hangs doesn't outlive the app.
        if let Some(mut child) = self.pw_dump.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Host for QtHost {
    fn wall_time(&self) -> f64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |since| since.as_secs_f64())
    }

    fn monotonic(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn start_timer(&mut self, timer: Timer) {
        self.schedule(|schedule| {
            schedule.starts += 1;
            let start = schedule.starts;
            schedule.deadlines.insert(timer, (Instant::now() + Duration::from_millis(timer.interval_ms()), start));
            schedule.live.insert(timer, start);
        });
    }

    fn stop_timer(&mut self, timer: Timer) {
        self.schedule(|schedule| {
            schedule.deadlines.remove(&timer);
            schedule.live.remove(&timer);
        });
    }

    fn poll_unit_soon(&mut self) {
        self.schedule(|schedule| schedule.soon.push(Instant::now() + POLL_UNIT_SOON));
    }

    fn pw_dump_running(&self) -> bool {
        self.pw_dump_running.load(Ordering::SeqCst)
    }

    fn start_pw_dump(&mut self) {
        self.pw_dump_running.store(true, Ordering::SeqCst);
        let (running, qt_thread) = (self.pw_dump_running.clone(), self.qt_thread.clone());
        let spawned = Command::new("pw-dump")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map(|mut child| {
                let stdout = child.stdout.take();
                *self.pw_dump.lock().unwrap() = Some(child);
                stdout
            });
        let slot = self.pw_dump.clone();
        std::thread::spawn(move || {
            let result = spawned.and_then(|stdout| {
                let mut output = Vec::new();
                if let Some(mut stdout) = stdout {
                    stdout.read_to_end(&mut output)?;
                }
                let child = slot.lock().unwrap().take();
                child.map(|mut child| Ok((child.wait()?, output))).transpose()
            });
            let result = match result {
                // The app quit meanwhile and killed it.
                Ok(None) => return,
                Ok(Some(finished)) => Ok(finished),
                Err(error) => Err(error),
            };
            queue(&qt_thread, move |receiver| {
                running.store(false, Ordering::SeqCst);
                match result {
                    // It couldn't be started.
                    Err(_) => receiver.on_pw_dump_error(),
                    Ok((status, output)) => {
                        if status.code().is_none() {
                            receiver.on_pw_dump_error(); // it crashed
                        }
                        receiver.on_pw_dump_finished(&output);
                    }
                }
            });
        });
    }

    fn pw_metadata(&mut self, args: &[String]) {
        let child = Command::new("pw-metadata")
            .args(["-n", "default", "--"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            std::thread::spawn(move || child.wait());
        }
    }

    fn open_file(&mut self, path: &Path) {
        shim::ffi::open_url(&QUrl::from_local_file(&QString::from(&*path.to_string_lossy())));
    }
}
