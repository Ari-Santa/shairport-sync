//! Mirror of shairport-sync's D-Bus state, and the receiver's output and
//! volume settings: everything the window shows and does.
//!
//! Nothing here knows about Qt or about a D-Bus library. The session bus is
//! reached through `Bus`, and timers, clocks and the PipeWire tools through
//! `Host`, so that all of it runs on one thread, from the calls the window and
//! those two make. What the window should hear about is collected as `Event`s.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

use crate::config::{self, Config};

const MPRIS_SERVICE: &str = "org.mpris.MediaPlayer2.ShairportSync";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const SPS_SERVICE: &str = "org.gnome.ShairportSync";
const SPS_PATH: &str = "/org/gnome/ShairportSync";
const SPS_IFACE: &str = "org.gnome.ShairportSync";
const SPS_REMOTE: &str = "org.gnome.ShairportSync.RemoteControl";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";

const SYSTEMD_SERVICE: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const SYSTEMD_UNIT: &str = "org.freedesktop.systemd1.Unit";
const SYSTEMD_SERVICE_IFACE: &str = "org.freedesktop.systemd1.Service";
const RECEIVER_UNIT: &str = "shairport-sync.service";
const TIMING_UNIT: &str = "nqptp.service";

/// The (service, path) pairs whose PropertiesChanged signals mean the state
/// should be read again, and the name whose coming and going does too.
pub const WATCHED_OBJECTS: [(&str, &str); 2] = [(MPRIS_SERVICE, MPRIS_PATH), (SPS_SERVICE, SPS_PATH)];
pub const WATCHED_SERVICE: &str = MPRIS_SERVICE;

/// How long a D-Bus call may take.
pub const DBUS_TIMEOUT_MS: u64 = 1000;
const OUTPUT_POLL_MS: u64 = 3000;
/// A picked output is saved once the picker has stayed on it this long, so
/// stepping through the list (wheel, arrow keys) saves and applies it only once.
const OUTPUT_COMMIT_MS: u64 = 1000;
/// How long after the receiver starts its PipeWire connections are taken as
/// settled, and how long it must run after an automatic restart to count as
/// working again.
const SETTLE_S: f64 = 5.0;
const FAILING_S: f64 = 10.0;
/// The app doesn't restart a receiver that has run for less than this, so that
/// its restarts (one per output picked while nothing plays, say) stay well
/// inside the units' start limit of 10 starts a minute.
const MIN_RUN_S: f64 = 10.0;
/// The PipeWire client shairport-sync opens to set a sink's volume
/// (pipewire.mixer_type = "sink"); it closes it again if it can't.
const SINK_VOLUME_CLIENT: &str = "shairport-sync-sink";

/// A value in a D-Bus message, as far as the app tells them apart.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Double(f64),
    /// A string or an object path.
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Val>),
    Dict(HashMap<String, Val>),
    /// Anything else.
    Other,
}

impl Val {
    fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_dict(&self) -> Option<&HashMap<String, Val>> {
        match self {
            Val::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// The number this is, or 0.
    fn as_f64(&self) -> f64 {
        match *self {
            Val::Bool(b) => f64::from(u8::from(b)),
            Val::Int(i) => i as f64,
            Val::UInt(u) => u as f64,
            Val::Double(d) => d,
            _ => 0.0,
        }
    }

    fn as_int(&self) -> Option<i64> {
        match *self {
            Val::Int(i) => Some(i),
            Val::UInt(u) => i64::try_from(u).ok(),
            _ => None,
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Val::Bool(b) => *b,
            Val::Int(i) => *i != 0,
            Val::UInt(u) => *u != 0,
            Val::Double(d) => *d != 0.0,
            Val::Str(s) => !s.is_empty(),
            Val::Bytes(b) => !b.is_empty(),
            Val::List(l) => !l.is_empty(),
            Val::Dict(d) => !d.is_empty(),
            Val::Other => true,
        }
    }

    /// As text: MPRIS lists (e.g. xesam:artist) are shown joined, scalars as-is.
    fn display(&self) -> String {
        match self {
            Val::Str(s) => s.clone(),
            Val::Bool(b) => (if *b { "True" } else { "False" }).to_string(),
            Val::Int(i) => i.to_string(),
            Val::UInt(u) => u.to_string(),
            Val::Double(d) => d.to_string(),
            Val::List(items) => {
                let shown: Vec<String> = items.iter().filter(|v| v.is_truthy()).map(Val::display).collect();
                shown.join(", ")
            }
            Val::Bytes(_) | Val::Dict(_) | Val::Other => String::new(),
        }
    }
}

type Props = HashMap<String, Val>;

fn text(props: &Props, key: &str) -> String {
    props.get(key).and_then(Val::as_str).unwrap_or_default().to_string()
}

fn shown(props: &Props, key: &str) -> String {
    props.get(key).map(Val::display).unwrap_or_default()
}

fn number(props: &Props, key: &str) -> f64 {
    props.get(key).map_or(0.0, Val::as_f64)
}

/// The session bus.
pub trait Bus {
    /// Call a method and wait for its reply, for at most `DBUS_TIMEOUT_MS`.
    /// Variants in the reply are replaced by what they hold. The error is the
    /// D-Bus error's message or, if it has none, its name.
    fn call(&self, service: &str, path: &str, interface: &str, method: &str, args: &[Val]) -> Result<Vec<Val>, String>;
}

/// The app's timers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    /// Saves a picked output once the picker has stayed on it.
    Commit,
    /// Looks at PipeWire's outputs and at the config file.
    Output,
    /// Coalesces bursts of PropertiesChanged signals into one refresh.
    Refresh,
    Position,
    Unit,
}

impl Timer {
    pub fn interval_ms(self) -> u64 {
        match self {
            Timer::Commit => OUTPUT_COMMIT_MS,
            Timer::Output => OUTPUT_POLL_MS,
            Timer::Refresh => 50,
            Timer::Position => 500,
            Timer::Unit => 2000,
        }
    }

    /// Whether it fires once per start, or again and again until stopped.
    pub fn single_shot(self) -> bool {
        matches!(self, Timer::Commit | Timer::Refresh)
    }
}

/// What the receiver needs of the program around it. Whatever these start
/// reports back through `Receiver::on_timer`, `on_poll_unit_soon`,
/// `on_pw_dump_finished` and `on_pw_dump_error`, on the same thread.
pub trait Host {
    /// Seconds since the Unix epoch.
    fn wall_time(&self) -> f64;
    /// Seconds on a clock that never goes back.
    fn monotonic(&self) -> f64;
    /// Start a timer, or start it again if it is running.
    fn start_timer(&mut self, timer: Timer);
    fn stop_timer(&mut self, timer: Timer);
    /// Have `Receiver::on_poll_unit_soon` called once in 300 ms, once for
    /// every call of this.
    fn poll_unit_soon(&mut self);
    fn pw_dump_running(&self) -> bool;
    /// Run `pw-dump`; its output goes to `Receiver::on_pw_dump_finished`.
    fn start_pw_dump(&mut self);
    /// Run `pw-metadata -n default -- <args>` and let it be.
    fn pw_metadata(&mut self, args: &[String]);
    /// Open a file in the user's editor for it.
    fn open_file(&mut self, path: &Path);
}

/// What the window should hear about, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    StateChanged,
    PositionChanged,
    OutputsChanged,
    DeviceVolumeChanged,
    Error(String),
    Notice(String),
}

/// What shairport-sync says about itself and about what is playing.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub online: bool,
    pub receiver_name: String,
    pub version: String,
    /// "AirPlay 2", or "AirPlay" for classic AirPlay.
    pub protocol: String,
    pub active: bool,
    pub client_name: String,
    pub client_address: String,
    pub playback_status: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    pub duration: f64,
    pub volume: f64,
    pub source_format: String,
    pub output_format: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            online: false,
            receiver_name: String::new(),
            version: String::new(),
            protocol: String::new(),
            active: false,
            client_name: String::new(),
            client_address: String::new(),
            playback_status: "Stopped".to_string(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            art_url: String::new(),
            duration: 0.0,
            volume: 0.0,
            source_format: String::new(),
            output_format: String::new(),
        }
    }
}

/// An audio output: a PipeWire sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sink {
    /// Its node.name.
    pub name: String,
    pub description: String,
}

/// What a receiver process started with.
#[derive(Debug, Clone, PartialEq)]
struct Running {
    process: Option<String>,
    target: String,
    device_volume: bool,
    /// The default output when it started.
    default_sink: Option<String>,
    /// Whether its device-volume target was there then.
    target_present: Option<bool>,
}

/// A look at PipeWire: when, the sinks' names and the default sink.
type Look = (f64, HashSet<String>, String);

/// How the running receiver handles Use Device Volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceVolumeState {
    /// It sets its output device's volume.
    On,
    /// It started with Use Device Volume but turns the audio down itself (it
    /// gives up if it can't find the device as it starts).
    Fallback,
    /// It didn't start with it, or this isn't known yet.
    Unknown,
}

/// A PipeWire object's id, or the subject of a metadata entry.
fn json_id(value: Option<&Json>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| value.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
}

fn json_text<'a>(props: &'a Json, key: &str) -> &'a str {
    props.get(key).and_then(Json::as_str).unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Receiver {
    bus: Box<dyn Bus>,
    host: Box<dyn Host>,
    config: Config,
    /// What the running receiver process started with, kept for the app's
    /// next run (the receiver only reads its settings when it starts).
    state_file: PathBuf,
    events: Vec<Event>,

    state: State,
    position: f64,

    // The receiver's unit. unit_state is its ActiveState, or "failing" while
    // systemd keeps restarting it after it stopped by itself.
    unit_state: String,
    active_state: String,
    failed_unit: &'static str,
    unit_seen: bool,
    /// Which run of the unit is the running receiver.
    process: Option<String>,
    main_pid: u64,
    /// When that run started, as wall time.
    started: f64,
    /// Whether the app saw it start.
    observed: bool,
    n_restarts: Option<i64>,
    /// Monotonic time of the last automatic restart seen.
    crashed: Option<f64>,

    // Output selection and volume mode. The lasting choices are
    // pipewire.sink_target and pipewire.mixer_type in the user's config,
    // which shairport-sync only reads at startup, so a change is applied by
    // restarting it once no sender is connected. A stream that is already
    // playing is moved to a new output straight away through PipeWire
    // metadata, unless the receiver sets its output device's volume: it sets
    // that of the device it started with until it restarts, so the stream
    // stays there until then.
    pipewire_ok: bool,
    pw_started: f64,
    /// When the last pw-dump was taken.
    pw_time: f64,
    /// The last two looks.
    pw_history: Vec<Look>,
    sinks: Vec<Sink>,
    default_sink: String,
    stream_id: Option<i64>,
    pinned_stream: Option<i64>,
    sink_volume_client: bool,
    output_target: String,
    output_label: String,
    device_volume: bool,
    /// (node.name, label) picked but not saved yet.
    choice: Option<(String, String)>,
    /// What the running receiver started with.
    running: Option<Running>,
    restart_pending: bool,
    /// The run a restart was last asked for.
    restart_sent: Option<String>,
    device_volume_warning: String,
}

impl Receiver {
    /// A receiver that has looked at nothing yet; see `start`.
    pub fn new(bus: Box<dyn Bus>, host: Box<dyn Host>, config: Config, state_file: PathBuf) -> Self {
        let (output_target, output_label) = config.read_sink_target();
        let device_volume = config.read_device_volume();
        Receiver {
            bus,
            host,
            config,
            state_file,
            events: Vec::new(),
            state: State::default(),
            position: 0.0,
            unit_state: "unknown".to_string(),
            active_state: "unknown".to_string(),
            failed_unit: RECEIVER_UNIT,
            unit_seen: false,
            process: None,
            main_pid: 0,
            started: 0.0,
            observed: false,
            n_restarts: None,
            crashed: None,
            pipewire_ok: false,
            pw_started: 0.0,
            pw_time: 0.0,
            pw_history: Vec::new(),
            sinks: Vec::new(),
            default_sink: String::new(),
            stream_id: None,
            pinned_stream: None,
            sink_volume_client: false,
            output_target,
            output_label,
            device_volume,
            choice: None,
            running: None,
            restart_pending: false,
            restart_sent: None,
            device_volume_warning: String::new(),
        }
    }

    /// Start the timers and take a first look at everything.
    pub fn start(&mut self) {
        self.host.start_timer(Timer::Output);
        self.refresh_outputs();
        self.host.start_timer(Timer::Unit);
        self.read_unit();
        self.refresh();
    }

    /// What has happened since this was last called.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn emit(&mut self, event: Event) {
        self.events.push(event);
    }

    // ---- what the window shows -----------------------------------------

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn unit_state(&self) -> &str {
        &self.unit_state
    }

    /// The unit whose log tells why the receiver isn't working, without ".service".
    pub fn failed_unit(&self) -> &str {
        self.failed_unit.strip_suffix(".service").unwrap_or(self.failed_unit)
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn config_file(&self) -> &Path {
        &self.config.file
    }

    pub fn output_switching_available(&self) -> bool {
        self.pipewire_ok && self.config.uses_pipewire_backend()
    }

    pub fn outputs(&self) -> &[Sink] {
        &self.sinks
    }

    pub fn default_output(&self) -> &str {
        self.sinks.iter().find(|s| s.name == self.default_sink).map_or("", |s| &s.description)
    }

    /// The output picked: one waiting to be saved, or else the saved one.
    pub fn output_target(&self) -> &str {
        self.choice.as_ref().map_or(&self.output_target, |choice| &choice.0)
    }

    pub fn output_target_label(&self) -> &str {
        self.choice.as_ref().map_or(&self.output_label, |choice| &choice.1)
    }

    pub fn device_volume_available(&self) -> bool {
        self.config.uses_pipewire_backend()
    }

    pub fn device_volume(&self) -> bool {
        self.device_volume
    }

    /// Why Use Device Volume is on but the running receiver turns the audio
    /// down itself, or "".
    pub fn device_volume_warning(&self) -> &str {
        &self.device_volume_warning
    }

    // ---- D-Bus helpers -------------------------------------------------

    fn call(&self, service: &str, path: &str, interface: &str, method: &str, args: &[Val]) -> Result<Vec<Val>, String> {
        self.bus.call(service, path, interface, method, args)
    }

    fn get_all(&self, service: &str, path: &str, interface: &str) -> Option<Props> {
        let reply = self.call(service, path, PROPS_IFACE, "GetAll", &[Val::Str(interface.to_string())]).ok()?;
        match reply.into_iter().next()? {
            Val::Dict(props) => Some(props),
            _ => Some(Props::new()),
        }
    }

    // ---- state refresh -------------------------------------------------

    /// A PropertiesChanged signal came from one of `WATCHED_OBJECTS`.
    pub fn on_properties_changed(&mut self) {
        self.host.start_timer(Timer::Refresh);
    }

    /// `WATCHED_SERVICE` appeared on the bus.
    pub fn on_service_registered(&mut self) {
        self.host.start_timer(Timer::Refresh);
    }

    /// `WATCHED_SERVICE` left the bus.
    pub fn on_service_unregistered(&mut self) {
        self.refresh();
    }

    /// A timer started with `Host::start_timer` fired.
    pub fn on_timer(&mut self, timer: Timer) {
        match timer {
            Timer::Commit => self.commit_output(),
            Timer::Output => self.refresh_outputs(),
            Timer::Refresh => self.refresh(),
            Timer::Position => self.poll_position(),
            Timer::Unit => self.poll_unit(),
        }
    }

    /// The 300 ms after a `Host::poll_unit_soon` are over.
    pub fn on_poll_unit_soon(&mut self) {
        self.poll_unit();
    }

    pub fn refresh(&mut self) {
        let Some(player) = self.get_all(MPRIS_SERVICE, MPRIS_PATH, MPRIS_PLAYER) else {
            self.set_state(State::default());
            return;
        };
        let core = self.get_all(SPS_SERVICE, SPS_PATH, SPS_IFACE).unwrap_or_default();
        let remote = self.get_all(SPS_SERVICE, SPS_PATH, SPS_REMOTE).unwrap_or_default();
        let no_metadata = Props::new();
        let meta = player.get("Metadata").and_then(Val::as_dict).unwrap_or(&no_metadata);

        self.set_state(State {
            online: true,
            receiver_name: text(&core, "ServiceName"),
            version: text(&core, "Version"),
            protocol: text(&core, "Protocol"),
            active: core.get("Active").is_some_and(Val::is_truthy),
            client_name: text(&remote, "ClientName"),
            client_address: text(&remote, "Client"),
            playback_status: player
                .get("PlaybackStatus")
                .map_or_else(|| "Stopped".to_string(), |status| status.as_str().unwrap_or_default().to_string()),
            title: shown(meta, "xesam:title"),
            artist: shown(meta, "xesam:artist"),
            album: shown(meta, "xesam:album"),
            art_url: shown(meta, "mpris:artUrl"),
            duration: number(meta, "mpris:length") / 1e6,
            volume: number(&player, "Volume"),
            source_format: text(&core, "SourceFormat"),
            output_format: text(&core, "OutputFormat"),
        });
        self.poll_position();
    }

    fn set_state(&mut self, new_state: State) {
        if new_state != self.state {
            self.state = new_state;
            self.emit(Event::StateChanged);
        }
        if self.state.playback_status == "Playing" {
            self.host.start_timer(Timer::Position);
        } else {
            self.host.stop_timer(Timer::Position);
        }
        if !self.state.online && self.position != 0.0 {
            self.position = 0.0;
            self.emit(Event::PositionChanged);
        }
        self.apply_pending_restart(false);
    }

    /// A sender is connected: playing, or paused (an AirPlay 2 sender that
    /// pauses goes inactive but stays connected), until it disconnects and
    /// MPRIS goes back to Stopped. The window shows this as streaming too.
    pub fn in_session(&self) -> bool {
        self.state.online && (self.state.active || self.state.playback_status != "Stopped")
    }

    fn session_over(&self) -> bool {
        self.state.online && !self.in_session()
    }

    fn poll_position(&mut self) {
        if !self.state.online {
            return;
        }
        let args = [Val::Str(MPRIS_PLAYER.to_string()), Val::Str("Position".to_string())];
        let Ok(reply) = self.call(MPRIS_SERVICE, MPRIS_PATH, PROPS_IFACE, "Get", &args) else {
            return;
        };
        let Some(value) = reply.first() else {
            return;
        };
        let seconds = value.as_f64() / 1e6;
        if (seconds - self.position).abs() > 0.05 {
            self.position = seconds;
            self.emit(Event::PositionChanged);
        }
    }

    // ---- the receiver's unit -------------------------------------------

    fn unit_properties(&self, unit: &str, interfaces: &[&str]) -> Result<Props, String> {
        let reply = self.call(SYSTEMD_SERVICE, SYSTEMD_PATH, SYSTEMD_MANAGER, "LoadUnit", &[Val::Str(unit.to_string())])?;
        let path = reply.first().map(Val::display).ok_or_else(|| "empty reply".to_string())?;
        let mut props = Props::new();
        for interface in interfaces {
            props.extend(self.get_all(SYSTEMD_SERVICE, &path, interface).unwrap_or_default());
        }
        Ok(props)
    }

    /// Update what is known about the receiver's unit; true if unit_state or
    /// failed_unit changed.
    fn read_unit(&mut self) -> bool {
        let unit = self.unit_properties(RECEIVER_UNIT, &[SYSTEMD_UNIT, SYSTEMD_SERVICE_IFACE]).unwrap_or_default();
        let state = unit.get("ActiveState").map_or_else(|| "unknown".to_string(), Val::display);
        // New with every start.
        let invocation = match unit.get("InvocationID") {
            Some(Val::Bytes(bytes)) => hex(bytes),
            _ => String::new(),
        };
        let main_pid = unit.get("MainPID").and_then(Val::as_int).and_then(|pid| u64::try_from(pid).ok()).unwrap_or(0);
        let started_usec = [number(&unit, "ExecMainStartTimestamp"), number(&unit, "ActiveEnterTimestamp")]
            .into_iter()
            .find(|&usec| usec != 0.0)
            .unwrap_or(0.0);
        let started = started_usec / 1e6;
        let process = if !invocation.is_empty() {
            invocation
        } else if main_pid != 0 {
            format!("{main_pid}@{started:?}")
        } else {
            String::new()
        };

        let now = self.host.monotonic();
        let restarts = unit.get("NRestarts").and_then(Val::as_int);
        let auto_restarting = state == "activating" && text(&unit, "SubState") == "auto-restart";
        let restarted = matches!((restarts, self.n_restarts), (Some(new), Some(old)) if new > old);
        if auto_restarting || restarted {
            self.crashed = Some(now);
        }
        self.n_restarts = restarts;

        if state == "active" && self.process.as_deref() != Some(process.as_str()) {
            // A receiver process the app hasn't seen yet. It was seen starting
            // if the app was already watching the unit, or it has only just
            // started.
            let wall = self.host.wall_time();
            let since = if started != 0.0 { started } else { wall };
            self.observed = self.unit_seen || wall - since < SETTLE_S;
            self.process = Some(process);
            self.main_pid = main_pid;
            self.started = since;
            self.pinned_stream = None;
            self.running = None;
        }
        if state == "active" && self.running.is_none() {
            self.adopt_receiver(); // again while the config doesn't parse
        }
        self.unit_seen = self.unit_seen || state != "unknown";

        let mut shown = state.clone();
        let mut failed_unit = RECEIVER_UNIT;
        if state == "failed" {
            // Its own failure is the one to show.
        } else if state != "inactive"
            && state != "unknown"
            && self.crashed.is_some_and(|crashed| now - crashed < FAILING_S)
            && !(state == "active" && self.host.wall_time() - self.started >= FAILING_S)
        {
            shown = "failing".to_string();
        } else if state != "active" {
            // The receiver can't start while the timing service it needs fails.
            let timing = self.unit_properties(TIMING_UNIT, &[SYSTEMD_UNIT]).unwrap_or_default();
            let timing_state = text(&timing, "ActiveState");
            if timing_state == "failed" {
                shown = "failed".to_string();
                failed_unit = TIMING_UNIT;
            } else if timing_state == "activating" && text(&timing, "SubState") == "auto-restart" {
                shown = "failing".to_string();
                failed_unit = TIMING_UNIT;
            }
        }

        self.active_state = state;
        if shown == self.unit_state && failed_unit == self.failed_unit {
            return false;
        }
        self.unit_state = shown;
        self.failed_unit = failed_unit;
        true
    }

    fn poll_unit(&mut self) {
        if self.read_unit() {
            self.emit(Event::StateChanged);
        }
        if self.active_state == "active" && !self.state.online {
            // Its objects may not have been exported yet when its names
            // appeared on the bus.
            self.refresh();
        }
        self.update_device_volume_warning();
        self.apply_pending_restart(false);
    }

    /// What a receiver process started with: what an earlier run of the app
    /// saved for that very process, or else the config as it is now, which
    /// the receiver read when it started (unless it was changed since while
    /// the app wasn't open: that can't be told apart from an unrelated edit).
    fn adopt_receiver(&mut self) {
        self.running = self.load_record();
        if self.running.is_some() {
            return;
        }
        self.reload_settings();
        if !self.config.parses() {
            return; // it didn't start with this; wait for the file to be fixed
        }
        self.running = Some(Running {
            process: self.process.clone(),
            target: self.output_target.clone(),
            device_volume: self.device_volume,
            default_sink: None,
            target_present: None,
        });
        self.learn_start_conditions();
        self.save_record();
    }

    /// Fill in the start conditions of a newly seen receiver process from the
    /// first look at PipeWire after it started; true if they changed.
    fn learn_start_conditions(&mut self) -> bool {
        let Some(running) = &mut self.running else {
            return false;
        };
        if !self.pipewire_ok || running.default_sink.is_some() || self.observed && self.pw_time < self.started {
            return false;
        }
        // If the app didn't see it start, the default output it started with
        // is taken to be the one now.
        running.default_sink = Some(self.default_sink.clone());
        if self.observed {
            // There in the first look after it started, and in the last one
            // before: a device that appeared around then may have come too
            // late for the receiver.
            let present = |look: &Look| {
                let (_, names, default_sink) = look;
                names.contains(if running.target.is_empty() { default_sink } else { &running.target })
            };
            let before = self.pw_history.iter().filter(|look| look.0 < self.started).next_back();
            running.target_present = Some(self.pw_history.last().into_iter().chain(before).all(present));
        }
        true
    }

    fn load_record(&self) -> Option<Running> {
        let record: Json = serde_json::from_slice(&fs::read(&self.state_file).ok()?).ok()?;
        let record = record.as_object()?;
        let process = self.process.as_deref().filter(|process| !process.is_empty())?;
        if record.get("process").and_then(Json::as_str) != Some(process) {
            return None;
        }
        Some(Running {
            process: self.process.clone(),
            target: record.get("target")?.as_str()?.to_string(),
            device_volume: record.get("device_volume")?.as_bool()?,
            default_sink: record.get("default_sink").and_then(Json::as_str).map(str::to_string),
            target_present: record.get("target_present").and_then(Json::as_bool),
        })
    }

    fn save_record(&self) {
        let Some(running) = &self.running else {
            return;
        };
        if self.process.as_deref().unwrap_or_default().is_empty() {
            return;
        }
        let record = serde_json::json!({
            "process": running.process,
            "target": running.target,
            "device_volume": running.device_volume,
            "default_sink": running.default_sink,
            "target_present": running.target_present,
        });
        // A failure is let be: the record only lets a reopened app finish
        // what this one started.
        let _ = (|| -> std::io::Result<()> {
            if let Some(dir) = self.state_file.parent() {
                fs::create_dir_all(dir)?;
            }
            let (mut file, tmp) = config::create_temp_file(&self.state_file)?;
            let written = file
                .write_all(record.to_string().as_bytes())
                .and_then(|()| file.flush())
                .and_then(|()| fs::rename(&tmp, &self.state_file));
            if written.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            written
        })();
    }

    // ---- audio outputs -------------------------------------------------

    pub fn refresh_outputs(&mut self) {
        if self.reload_settings() {
            self.output_changed();
        }
        if !self.host.pw_dump_running() {
            self.pw_started = self.host.wall_time();
            self.host.start_pw_dump();
        }
        self.apply_pending_restart(false);
    }

    /// `pw-dump` couldn't be run, or it crashed.
    pub fn on_pw_dump_error(&mut self) {
        if self.pipewire_ok {
            self.pipewire_ok = false;
            self.emit(Event::OutputsChanged);
        }
    }

    /// Whether a PipeWire client belongs to the running receiver.
    fn is_receiver(&self, props: &Json) -> bool {
        if json_text(props, "application.process.binary") != "shairport-sync" {
            return false;
        }
        if self.main_pid == 0 {
            return true;
        }
        match props.get("application.process.id") {
            Some(Json::String(pid)) => *pid == self.main_pid.to_string(),
            Some(pid) => pid.as_u64() == Some(self.main_pid) || pid.as_f64() == Some(self.main_pid as f64),
            None => false,
        }
    }

    /// `pw-dump` ended, having written `output`.
    pub fn on_pw_dump_finished(&mut self, output: &[u8]) {
        let output = if output.is_empty() { b"[]" } else { output };
        let Ok(objects) = serde_json::from_slice::<Json>(output) else {
            return;
        };
        let objects = objects.as_array().map(Vec::as_slice).unwrap_or_default();
        if objects.is_empty() {
            // PipeWire isn't running (it always lists itself).
            self.on_pw_dump_error();
            return;
        }

        let mut stream_clients: HashSet<Option<i64>> = HashSet::new();
        let mut sink_volume_client = false;
        let mut nodes: Vec<(Option<i64>, &Json)> = Vec::new();
        let mut default_sink = String::new();
        let mut stream_targets: HashMap<Option<i64>, &Json> = HashMap::new();
        let no_props = Json::Null;
        for object in objects {
            let kind = json_text(object, "type");
            let props = object.get("info").and_then(|info| info.get("props")).unwrap_or(&no_props);
            match kind {
                "PipeWire:Interface:Client" => {
                    if !self.is_receiver(props) {
                        continue;
                    }
                    if json_text(props, "application.name") == SINK_VOLUME_CLIENT {
                        sink_volume_client = true;
                    } else if props.get("client.api").is_none() {
                        // The pipewire backend's stream. With the pa or alsa
                        // backend the receiver plays through pipewire-pulse or
                        // pipewire-alsa, whose clients have a client.api; their
                        // output is the system mixer's to choose, not the app's.
                        stream_clients.insert(json_id(object.get("id")));
                    }
                }
                "PipeWire:Interface:Node" => nodes.push((json_id(object.get("id")), props)),
                "PipeWire:Interface:Metadata" => {
                    if object.get("props").map(|props| json_text(props, "metadata.name")) != Some("default") {
                        continue;
                    }
                    let entries = object.get("metadata").and_then(Json::as_array).map(Vec::as_slice).unwrap_or_default();
                    for entry in entries {
                        let key = json_text(entry, "key");
                        let subject = json_id(entry.get("subject"));
                        if subject == Some(0) && key == "default.audio.sink" {
                            default_sink = match entry.get("value") {
                                Some(value) if value.is_object() => json_text(value, "name").to_string(),
                                _ => String::new(),
                            };
                        } else if key == "target.object" {
                            stream_targets.insert(subject, entry.get("value").unwrap_or(&no_props));
                        }
                    }
                }
                _ => {}
            }
        }

        let mut sinks: Vec<Sink> = nodes
            .iter()
            .filter(|(_, props)| json_text(props, "media.class") == "Audio/Sink")
            .map(|(_, props)| {
                let name = json_text(props, "node.name");
                let description = ["node.description", "node.nick"]
                    .into_iter()
                    .map(|key| json_text(props, key))
                    .find(|text| !text.is_empty())
                    .unwrap_or(name);
                Sink { name: name.to_string(), description: description.to_string() }
            })
            .collect();
        sinks.sort_by_cached_key(|sink| sink.description.to_lowercase());
        let stream_id = nodes
            .iter()
            .find(|(_, props)| {
                json_text(props, "media.class") == "Stream/Output/Audio"
                    && stream_clients.contains(&json_id(props.get("client.id")))
            })
            .and_then(|(id, _)| *id);

        let new_stream = stream_id.is_some() && stream_id != self.stream_id;
        if stream_id != self.stream_id {
            self.pinned_stream = None; // PipeWire reuses the ids of objects that have gone
        }
        self.stream_id = stream_id;
        self.sink_volume_client = sink_volume_client;
        self.pw_time = self.pw_started;
        let changed = !self.pipewire_ok || sinks != self.sinks || default_sink != self.default_sink;
        self.pipewire_ok = true;
        self.sinks = sinks;
        self.default_sink = default_sink;
        let names = self.sinks.iter().map(|sink| sink.name.clone()).collect();
        if self.pw_history.len() > 1 {
            self.pw_history.remove(0);
        }
        self.pw_history.push((self.pw_time, names, self.default_sink.clone()));
        if self.learn_start_conditions() {
            self.save_record();
        }
        if changed {
            self.emit(Event::OutputsChanged);
        }

        if new_stream {
            self.place_new_stream(stream_targets.get(&stream_id).copied());
        }
        self.keep_on_volume_device();
        self.update_device_volume_warning();
        self.apply_pending_restart(false);
    }

    fn sink_present(&self, name: &str) -> bool {
        !name.is_empty() && self.sinks.iter().any(|sink| sink.name == name)
    }

    fn output_name(&self, name: &str) -> String {
        if name.is_empty() {
            return "the default output".to_string();
        }
        if name == self.output_target && !self.output_label.is_empty() {
            return self.output_label.clone();
        }
        self.sinks.iter().find(|sink| sink.name == name).map_or(name, |sink| &sink.description).to_string()
    }

    /// A new receiver stream: a session started, or the receiver reconnected
    /// its stream. `restored` is the target PipeWire's metadata has for it.
    fn place_new_stream(&mut self, restored: Option<&Json>) {
        if !self.config.uses_pipewire_backend() {
            return;
        }
        let moved_away = match restored {
            None | Some(Json::Null) => false,
            Some(Json::String(target)) => target != "-1",
            Some(target) => target.as_f64() != Some(-1.0),
        };
        let running_target = self.running.as_ref().map(|running| running.target.clone());
        if self.sets_device_volume() {
            // It plays where the volume it sets is: on the target it started
            // with, or with Follow System on the default it started with
            // (which keep_on_volume_device sees to if the default has changed
            // since).
            if running_target.as_deref() == Some("") && moved_away && self.volume_device_to_keep().is_none() {
                // WirePlumber restored a target from someone moving it
                // elsewhere; put it right.
                self.move_stream(self.stream_id, "");
            }
        } else if running_target.is_some_and(|target| target != self.output_target) {
            // Its process still has an older choice; it restarts to take the
            // new one once the session is over.
            self.move_stream(self.stream_id, &self.output_target.clone());
        } else if self.output_target.is_empty() && moved_away {
            // WirePlumber restored a target from someone moving it elsewhere
            // while we follow the default; put it right.
            self.move_stream(self.stream_id, "");
        }
    }

    /// The output to keep the receiver's stream on, by name, because the
    /// receiver sets its volume, or None. With Follow System, a receiver that
    /// sets its device's volume sets that of the default output it started
    /// with, so when the default changes its stream is kept on that device
    /// until the receiver can restart.
    fn volume_device_to_keep(&self) -> Option<String> {
        if self.device_volume_state() != DeviceVolumeState::On {
            return None;
        }
        let running = self.running.as_ref()?;
        let default_sink = running.default_sink.as_deref().filter(|sink| !sink.is_empty())?;
        if !running.target.is_empty() || default_sink == self.default_sink || !self.sink_present(default_sink) {
            return None;
        }
        Some(default_sink.to_string())
    }

    /// Once per stream, see `volume_device_to_keep`.
    fn keep_on_volume_device(&mut self) {
        if self.stream_id.is_none() || self.stream_id == self.pinned_stream {
            return;
        }
        let Some(device) = self.volume_device_to_keep() else {
            return;
        };
        self.pinned_stream = self.stream_id;
        self.move_stream(self.stream_id, &device);
        if self.in_session() {
            self.emit(Event::Notice(format!(
                "AirPlay keeps playing on {} until this session ends, because the sender's volume control \
                 sets the volume of that device.",
                self.output_name(&device)
            )));
        }
    }

    /// By node.name, or -1 for "the default": WirePlumber then moves the
    /// stream without remembering a per-app override that could outlive our
    /// choice.
    fn move_stream(&mut self, stream_id: Option<i64>, target: &str) {
        let stream = stream_id.map_or_else(|| "None".to_string(), |id| id.to_string());
        let target = if target.is_empty() { "-1" } else { target };
        self.host.pw_metadata(&[stream, "target.object".to_string(), target.to_string()]);
    }

    /// Pick an output. It is saved and applied once the choice has stayed put
    /// for `OUTPUT_COMMIT_MS`, or at `commit_output`.
    pub fn set_output(&mut self, name: &str) {
        if name == self.output_target() {
            return;
        }
        let mut label = self.sinks.iter().find(|sink| sink.name == name).map_or("", |sink| &sink.description);
        if label.is_empty() && name == self.output_target {
            label = &self.output_label;
        }
        let label = if name.is_empty() { String::new() } else { label.to_string() };
        self.choice = Some((name.to_string(), label));
        self.emit(Event::OutputsChanged);
        self.host.start_timer(Timer::Commit);
    }

    pub fn commit_output(&mut self) {
        if self.save_choice() {
            self.apply_pending_restart(false);
        }
    }

    /// The app is about to quit.
    pub fn quit(&mut self) {
        self.save_choice();
        self.apply_pending_restart(true);
    }

    /// Save a picked output; true if the saved output changed (here, or in
    /// the file since it was last read).
    fn save_choice(&mut self) -> bool {
        self.host.stop_timer(Timer::Commit);
        let Some((name, label)) = self.choice.take() else {
            return false;
        };
        let mut changed = self.reload_settings();
        if name != self.output_target {
            match self.config.write_sink_target(&name, &label) {
                Err(err) => self.emit(Event::Error(format!("Could not save the output choice: {err}"))),
                Ok(()) => {
                    self.output_target = name;
                    self.output_label = label;
                    changed = true;
                }
            }
        }
        self.emit(Event::OutputsChanged); // on failure, the picker goes back to the saved output
        if changed {
            self.output_changed();
        }
        changed
    }

    /// The saved output changed: move a playing stream to it.
    fn output_changed(&mut self) {
        if self.stream_id.is_none() || !self.config.uses_pipewire_backend() {
            return;
        }
        if self.sets_device_volume() {
            if self.in_session() {
                let volume_device = self
                    .running
                    .as_ref()
                    .map(|running| {
                        if running.target.is_empty() {
                            running.default_sink.clone().unwrap_or_default()
                        } else {
                            running.target.clone()
                        }
                    })
                    .unwrap_or_default();
                self.emit(Event::Notice(format!(
                    "AirPlay switches to {} when this session ends. Until then it stays on {}, because the \
                     sender's volume control sets the volume of that device.",
                    self.output_name(&self.output_target),
                    self.output_name(&volume_device)
                )));
            }
            return;
        }
        self.move_stream(self.stream_id, &self.output_target.clone());
    }

    pub fn set_device_volume(&mut self, enabled: bool) {
        if self.reload_settings() {
            self.output_changed();
        }
        if enabled == self.device_volume {
            self.emit(Event::DeviceVolumeChanged);
            return;
        }
        if let Err(err) = self.config.write_device_volume(enabled) {
            self.emit(Event::Error(format!("Could not save the volume setting: {err}")));
            self.emit(Event::DeviceVolumeChanged); // puts the menu's check mark back
            return;
        }
        self.device_volume = enabled;
        self.update_device_volume_warning();
        self.emit(Event::DeviceVolumeChanged);
        self.apply_pending_restart(false);
    }

    // ---- applying settings ---------------------------------------------

    /// Pick up changes to the output and volume settings made outside the
    /// app, e.g. with Edit Settings…; true if the saved output changed.
    fn reload_settings(&mut self) -> bool {
        if !self.config.parses() {
            return false; // e.g. saved halfway through an edit
        }
        let (target, label) = self.config.read_sink_target();
        let device_volume = self.config.read_device_volume();
        let changed = target != self.output_target;
        if target != self.output_target || label != self.output_label {
            self.output_target = target;
            self.output_label = label;
            self.emit(Event::OutputsChanged);
        }
        if device_volume != self.device_volume {
            self.device_volume = device_volume;
            self.emit(Event::DeviceVolumeChanged);
        }
        changed
    }

    /// Use Device Volume is a setting of the pipewire backend; it does
    /// nothing with the others.
    fn device_volume_state(&self) -> DeviceVolumeState {
        let started_with_it = self.running.as_ref().is_some_and(|running| running.device_volume);
        if !started_with_it
            || self.active_state != "active"
            || !self.pipewire_ok
            || self.pw_time < self.started + SETTLE_S
            || !self.config.uses_pipewire_backend()
        {
            return DeviceVolumeState::Unknown;
        }
        if self.sink_volume_client {
            DeviceVolumeState::On
        } else {
            DeviceVolumeState::Fallback
        }
    }

    /// As far as is known: a stream mustn't leave the device whose volume
    /// the sender's volume control sets.
    fn sets_device_volume(&self) -> bool {
        self.running.as_ref().is_some_and(|running| running.device_volume)
            && self.device_volume_state() != DeviceVolumeState::Fallback
    }

    /// Why the running receiver should be restarted, or None.
    fn restart_reason(&self) -> Option<&'static str> {
        let running = self.running.as_ref()?;
        if self.active_state != "active" || running.process != self.process {
            return None;
        }
        let device_volume_state = self.device_volume_state();
        let reason = if running.target != self.output_target || running.device_volume != self.device_volume {
            "the settings changed"
        } else if device_volume_state == DeviceVolumeState::Fallback
            && running.target_present != Some(true)
            && self.sink_present(if running.target.is_empty() { &self.default_sink } else { &running.target })
        {
            "the device for Use Device Volume is there now"
        } else if device_volume_state == DeviceVolumeState::On
            && running.target.is_empty()
            && !self.default_sink.is_empty()
            && running.default_sink.as_ref().is_some_and(|sink| *sink != self.default_sink)
        {
            "the default output changed"
        } else {
            return None;
        };
        (self.config.uses_pipewire_backend() && self.config.parses()).then_some(reason)
    }

    /// Restarting drops a connected sender, so wait until the session is
    /// over. While the receiver's D-Bus objects can't be reached, whether a
    /// sender is connected isn't known, so a change waits for its next start.
    /// A run that has only just started is left to run for `MIN_RUN_S` first,
    /// so that a quick run of changes restarts it fewer times, unless the app
    /// is quitting and won't be there to restart it later. Only the run the
    /// settings were compared with is restarted, and only while it runs: a
    /// receiver stopped meanwhile stays stopped.
    fn apply_pending_restart(&mut self, quitting: bool) {
        let restart_wanted =
            |receiver: &Self| receiver.restart_reason().is_some() && receiver.restart_sent != receiver.process;
        self.restart_pending = restart_wanted(self);
        if !self.restart_pending
            || !self.session_over()
            || !quitting && self.host.wall_time() - self.started < MIN_RUN_S
        {
            return;
        }
        if self.read_unit() {
            // Make sure that run is still the one going.
            self.emit(Event::StateChanged);
        }
        self.restart_pending = false;
        if restart_wanted(self) {
            self.manage_units("TryRestartUnit", &[RECEIVER_UNIT]);
        }
    }

    fn update_device_volume_warning(&mut self) {
        let mut warning = String::new();
        if self.device_volume && self.device_volume_state() == DeviceVolumeState::Fallback {
            if let Some(running) = &self.running {
                let device = if running.target.is_empty() { &self.default_sink } else { &running.target };
                let name = if device.is_empty() { "the output device".to_string() } else { self.output_name(device) };
                warning = if running.target_present == Some(true) {
                    format!(
                        "Use Device Volume isn't in effect: the receiver couldn't set the volume of {name}, so \
                         the sender's volume control turns the audio down digitally. Check its log with \
                         “journalctl --user -u shairport-sync”."
                    )
                } else if !self.sink_present(device) {
                    format!(
                        "Use Device Volume isn't in effect: {name} isn't connected, so the sender's volume \
                         control turns the audio down digitally. This app restarts the receiver to use the \
                         device's volume once it is connected and nothing is playing."
                    )
                } else {
                    format!(
                        "Use Device Volume isn't in effect yet: {name} wasn't connected when the receiver \
                         started. This app restarts the receiver to use the device's volume once nothing is \
                         playing."
                    )
                };
            }
        }
        if warning != self.device_volume_warning {
            self.device_volume_warning = warning;
            self.emit(Event::DeviceVolumeChanged);
        }
    }

    // ---- actions -------------------------------------------------------

    fn player_command(&mut self, method: &str, args: &[Val]) {
        if let Err(err) = self.call(MPRIS_SERVICE, MPRIS_PATH, MPRIS_PLAYER, method, args) {
            self.emit(Event::Error(format!("{method} failed: {err}")));
        }
    }

    pub fn play_pause(&mut self) {
        self.player_command("PlayPause", &[]);
    }

    pub fn next(&mut self) {
        self.player_command("Next", &[]);
    }

    pub fn previous(&mut self) {
        self.player_command("Previous", &[]);
    }

    pub fn set_volume(&mut self, value: f64) {
        self.player_command("SetVolume", &[Val::Double(value.min(1.0).max(0.0))]);
    }

    pub fn disconnect_sender(&mut self) {
        if let Err(err) = self.call(SPS_SERVICE, SPS_PATH, SPS_IFACE, "DropSession", &[]) {
            self.emit(Event::Error(format!("Could not disconnect: {err}")));
        }
    }

    fn manage_units(&mut self, method: &str, units: &[&str]) {
        // Whatever this does, the receiver reads the config when it next
        // starts, so the run going now has nothing left to apply.
        self.save_choice();
        self.restart_sent = self.process.clone();
        // Nor is a crash seen before still news: should the run this starts
        // crash too, NRestarts or SubState shows it again.
        self.crashed = None;
        self.n_restarts = None;
        if matches!(method, "StartUnit" | "RestartUnit") && matches!(self.unit_state.as_str(), "failed" | "failing") {
            // As `systemctl reset-failed` does, so that a unit that hit its
            // start limit can start again.
            for unit in [RECEIVER_UNIT, TIMING_UNIT] {
                let args = [Val::Str(unit.to_string())];
                let _ = self.call(SYSTEMD_SERVICE, SYSTEMD_PATH, SYSTEMD_MANAGER, "ResetFailedUnit", &args);
            }
        }
        for unit in units {
            let args = [Val::Str(unit.to_string()), Val::Str("replace".to_string())];
            if let Err(err) = self.call(SYSTEMD_SERVICE, SYSTEMD_PATH, SYSTEMD_MANAGER, method, &args) {
                let action = method.strip_prefix("Try").unwrap_or(method);
                let action = action.strip_suffix("Unit").unwrap_or(action).to_lowercase();
                self.emit(Event::Error(format!("Could not {action} the receiver: {err}")));
                break;
            }
        }
        self.host.poll_unit_soon();
    }

    pub fn start_receiver(&mut self) {
        // shairport-sync.service pulls in nqptp.service, so this brings up both.
        self.manage_units("StartUnit", &[RECEIVER_UNIT]);
    }

    pub fn stop_receiver(&mut self) {
        self.manage_units("StopUnit", &[RECEIVER_UNIT, TIMING_UNIT]);
    }

    pub fn restart_receiver(&mut self) {
        self.manage_units("RestartUnit", &[TIMING_UNIT, RECEIVER_UNIT]);
    }

    pub fn open_config(&mut self) {
        if let Err(err) = self.config.ensure_user_config() {
            self.emit(Event::Error(format!("Could not create {}: {err}", self.config.file.display())));
            return;
        }
        let file = self.config.file.clone();
        self.host.open_file(&file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// The world the receiver looks at: what is on the bus, the clocks, and
    /// a record of what it did.
    #[derive(Default)]
    struct World {
        /// GetAll replies by interface; an interface that is missing is an error.
        props: HashMap<&'static str, Props>,
        position_usec: i64,
        calls: Vec<String>,
        wall: f64,
        timers: HashSet<Timer>,
        poll_unit_soon: usize,
        pw_dump_running: bool,
        pw_metadata: Vec<Vec<String>>,
        opened: Vec<PathBuf>,
    }

    #[derive(Clone)]
    struct Fake(Rc<RefCell<World>>);

    impl Bus for Fake {
        fn call(&self, _service: &str, path: &str, interface: &str, method: &str, args: &[Val]) -> Result<Vec<Val>, String> {
            let mut world = self.0.borrow_mut();
            let arg = |i: usize| args.get(i).and_then(Val::as_str).unwrap_or_default().to_string();
            match (interface, method) {
                (PROPS_IFACE, "GetAll") => {
                    // The timing unit's properties are kept apart from the receiver's.
                    let key = if path == "/unit/nqptp.service" { "timing".to_string() } else { arg(0) };
                    let found = world.props.iter().find(|(name, _)| **name == key).map(|(_, props)| props.clone());
                    found.map(|props| vec![Val::Dict(props)]).ok_or_else(|| "no such interface".to_string())
                }
                (PROPS_IFACE, "Get") => Ok(vec![Val::Int(world.position_usec)]),
                (SYSTEMD_MANAGER, "LoadUnit") => Ok(vec![Val::Str(format!("/unit/{}", arg(0)))]),
                _ => {
                    world.calls.push(format!("{method}({})", args.iter().map(Val::display).collect::<Vec<_>>().join(", ")));
                    Ok(Vec::new())
                }
            }
        }
    }

    impl Host for Fake {
        fn wall_time(&self) -> f64 {
            self.0.borrow().wall
        }
        fn monotonic(&self) -> f64 {
            self.0.borrow().wall - 1000.0
        }
        fn start_timer(&mut self, timer: Timer) {
            self.0.borrow_mut().timers.insert(timer);
        }
        fn stop_timer(&mut self, timer: Timer) {
            self.0.borrow_mut().timers.remove(&timer);
        }
        fn poll_unit_soon(&mut self) {
            self.0.borrow_mut().poll_unit_soon += 1;
        }
        fn pw_dump_running(&self) -> bool {
            self.0.borrow().pw_dump_running
        }
        fn start_pw_dump(&mut self) {
            self.0.borrow_mut().pw_dump_running = true;
        }
        fn pw_metadata(&mut self, args: &[String]) {
            self.0.borrow_mut().pw_metadata.push(args.to_vec());
        }
        fn open_file(&mut self, path: &Path) {
            self.0.borrow_mut().opened.push(path.to_path_buf());
        }
    }

    fn props(pairs: &[(&str, Val)]) -> Props {
        pairs.iter().map(|(key, value)| (key.to_string(), value.clone())).collect()
    }

    fn s(text: &str) -> Val {
        Val::Str(text.to_string())
    }

    struct Setup {
        world: Rc<RefCell<World>>,
        receiver: Receiver,
        dir: PathBuf,
    }

    impl Drop for Setup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A receiver with the pipewire backend, looking at a world where
    /// nothing is on the bus yet.
    fn setup(name: &str, pipewire_settings: &str) -> Setup {
        let dir = std::env::temp_dir().join(format!("shairport-sync-gui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let config = Config {
            file: dir.join("shairport-sync.conf"),
            defaults_file: dir.join("defaults.conf"),
            system_file: dir.join("system.conf"),
        };
        let text = format!("general = {{ output_backend = \"pipewire\"; }};\npipewire =\n{{\n{pipewire_settings}}};\n");
        fs::write(&config.file, text).unwrap();
        let world = Rc::new(RefCell::new(World { wall: 5000.0, ..World::default() }));
        let fake = Fake(world.clone());
        let receiver = Receiver::new(Box::new(fake.clone()), Box::new(fake), config, dir.join("state/receiver.json"));
        Setup { world, receiver, dir }
    }

    /// Put a receiver unit that started at `started` on the bus.
    fn run_unit(world: &Rc<RefCell<World>>, invocation: u8, started: f64) {
        let mut world = world.borrow_mut();
        world.props.insert(
            SYSTEMD_UNIT,
            props(&[
                ("ActiveState", s("active")),
                ("SubState", s("running")),
                ("InvocationID", Val::Bytes(vec![invocation; 16])),
            ]),
        );
        world.props.insert(
            SYSTEMD_SERVICE_IFACE,
            props(&[
                ("MainPID", Val::UInt(4242)),
                ("NRestarts", Val::UInt(0)),
                ("ExecMainStartTimestamp", Val::UInt((started * 1e6) as u64)),
            ]),
        );
    }

    /// Put shairport-sync's own objects on the bus.
    fn go_online(world: &Rc<RefCell<World>>, status: &str, active: bool) {
        let mut world = world.borrow_mut();
        let metadata = props(&[
            ("xesam:title", s("A Song")),
            ("xesam:artist", Val::List(vec![s("One"), s(""), s("Two")])),
            ("xesam:album", s("An Album")),
            ("mpris:artUrl", s("file:///tmp/cover.jpg")),
            ("mpris:length", Val::Int(180_500_000)),
        ]);
        world.props.insert(
            MPRIS_PLAYER,
            props(&[("PlaybackStatus", s(status)), ("Volume", Val::Double(0.5)), ("Metadata", Val::Dict(metadata))]),
        );
        world.props.insert(
            SPS_IFACE,
            props(&[
                ("ServiceName", s("Den")),
                ("Version", s("5.6.2")),
                ("Protocol", s("AirPlay 2")),
                ("Active", Val::Bool(active)),
            ]),
        );
        world.props.insert(SPS_REMOTE, props(&[("ClientName", s("Phone")), ("Client", s("192.168.1.2"))]));
    }

    /// What pw-dump prints with two sinks and, with `stream`, the receiver's
    /// stream, and with `volume_client`, its client for the sink's volume.
    fn pw_dump(default_sink: &str, stream: bool, volume_client: bool, stream_target: Option<&str>) -> Vec<u8> {
        let mut objects = vec![
            serde_json::json!({"id": 0, "type": "PipeWire:Interface:Core", "info": {"props": {}}}),
            serde_json::json!({"id": 50, "type": "PipeWire:Interface:Node", "info": {"props": {
                "media.class": "Audio/Sink", "node.name": "usb", "node.description": "USB Audio"}}}),
            serde_json::json!({"id": 51, "type": "PipeWire:Interface:Node", "info": {"props": {
                "media.class": "Audio/Sink", "node.name": "hdmi", "node.nick": "HDMI"}}}),
            serde_json::json!({"id": 52, "type": "PipeWire:Interface:Node", "info": {"props": {
                "media.class": "Audio/Source", "node.name": "mic"}}}),
        ];
        let mut metadata = vec![serde_json::json!({"subject": 0, "key": "default.audio.sink", "value": {"name": default_sink}})];
        if stream {
            objects.push(serde_json::json!({"id": 70, "type": "PipeWire:Interface:Client", "info": {"props": {
                "application.process.binary": "shairport-sync", "application.process.id": 4242,
                "application.name": "Shairport Sync"}}}));
            objects.push(serde_json::json!({"id": 71, "type": "PipeWire:Interface:Node", "info": {"props": {
                "media.class": "Stream/Output/Audio", "client.id": 70}}}));
            // Another program's stream, and the receiver playing through pipewire-pulse.
            objects.push(serde_json::json!({"id": 80, "type": "PipeWire:Interface:Client", "info": {"props": {
                "application.process.binary": "shairport-sync", "application.process.id": "4242",
                "client.api": "pipewire-pulse"}}}));
            objects.push(serde_json::json!({"id": 81, "type": "PipeWire:Interface:Node", "info": {"props": {
                "media.class": "Stream/Output/Audio", "client.id": 80}}}));
        }
        if volume_client {
            objects.push(serde_json::json!({"id": 72, "type": "PipeWire:Interface:Client", "info": {"props": {
                "application.process.binary": "shairport-sync", "application.process.id": 4242,
                "application.name": SINK_VOLUME_CLIENT}}}));
        }
        if let Some(target) = stream_target {
            metadata.push(serde_json::json!({"subject": 71, "key": "target.object", "value": target}));
        }
        objects.push(serde_json::json!({"id": 60, "type": "PipeWire:Interface:Metadata",
            "props": {"metadata.name": "default"}, "metadata": metadata}));
        serde_json::to_vec(&objects).unwrap()
    }

    /// Let the receiver take a look at PipeWire at the world's time.
    fn look(setup: &mut Setup, output: &[u8]) {
        setup.world.borrow_mut().pw_dump_running = false;
        setup.receiver.refresh_outputs();
        setup.world.borrow_mut().pw_dump_running = false;
        setup.receiver.on_pw_dump_finished(output);
    }

    #[test]
    fn shows_what_is_playing() {
        let mut setup = setup("playing", "");
        setup.receiver.start();
        assert!(!setup.receiver.state().online);
        assert_eq!(setup.receiver.unit_state(), "unknown");
        assert!(setup.world.borrow().timers.contains(&Timer::Output) && setup.world.borrow().timers.contains(&Timer::Unit));
        assert!(!setup.world.borrow().timers.contains(&Timer::Position));
        setup.receiver.take_events();

        run_unit(&setup.world, 1, 4990.0);
        go_online(&setup.world, "Playing", true);
        setup.world.borrow_mut().position_usec = 61_000_000;
        setup.receiver.on_properties_changed();
        assert!(setup.world.borrow().timers.contains(&Timer::Refresh));
        setup.receiver.on_timer(Timer::Refresh);
        setup.receiver.on_timer(Timer::Unit);

        let state = setup.receiver.state().clone();
        assert!(state.online && state.active);
        assert_eq!(
            (state.receiver_name.as_str(), state.version.as_str(), state.protocol.as_str()),
            ("Den", "5.6.2", "AirPlay 2")
        );
        assert_eq!((state.title.as_str(), state.artist.as_str(), state.album.as_str()), ("A Song", "One, Two", "An Album"));
        assert_eq!((state.client_name.as_str(), state.client_address.as_str()), ("Phone", "192.168.1.2"));
        assert_eq!((state.duration, state.volume, setup.receiver.position()), (180.5, 0.5, 61.0));
        assert_eq!(setup.receiver.unit_state(), "active");
        assert!(setup.receiver.in_session());
        assert!(setup.world.borrow().timers.contains(&Timer::Position));
        let events = setup.receiver.take_events();
        assert!(events.contains(&Event::StateChanged) && events.contains(&Event::PositionChanged));

        // The service goes away.
        setup.world.borrow_mut().props.remove(MPRIS_PLAYER);
        setup.receiver.on_service_unregistered();
        assert_eq!(*setup.receiver.state(), State::default());
        assert_eq!(setup.receiver.position(), 0.0);
        assert!(!setup.world.borrow().timers.contains(&Timer::Position));
        assert_eq!(setup.receiver.take_events(), [Event::StateChanged, Event::PositionChanged]);
    }

    #[test]
    fn lists_the_outputs() {
        let mut setup = setup("outputs", "");
        setup.receiver.start();
        assert!(setup.world.borrow().pw_dump_running);
        assert!(!setup.receiver.output_switching_available());
        setup.receiver.take_events();

        look(&mut setup, &pw_dump("usb", false, false, None));
        assert!(setup.receiver.output_switching_available());
        let outputs: Vec<_> = setup.receiver.outputs().iter().map(|o| (o.name.as_str(), o.description.as_str())).collect();
        assert_eq!(outputs, [("hdmi", "HDMI"), ("usb", "USB Audio")]);
        assert_eq!(setup.receiver.default_output(), "USB Audio");
        assert_eq!(setup.receiver.take_events(), [Event::OutputsChanged]);

        look(&mut setup, &pw_dump("usb", false, false, None));
        assert_eq!(setup.receiver.take_events(), []);
        look(&mut setup, b"not json");
        assert!(setup.receiver.output_switching_available());
        look(&mut setup, b"");
        assert!(!setup.receiver.output_switching_available());
        assert_eq!(setup.receiver.take_events(), [Event::OutputsChanged]);
    }

    #[test]
    fn a_picked_output_is_saved_and_applied() {
        let mut setup = setup("pick", "");
        run_unit(&setup.world, 1, 4000.0);
        go_online(&setup.world, "Playing", true);
        setup.receiver.start();
        look(&mut setup, &pw_dump("usb", true, false, None));
        assert_eq!(setup.world.borrow().pw_metadata, Vec::<Vec<String>>::new());
        setup.receiver.take_events();

        // Picking waits for the commit timer; the stream moves when it fires.
        setup.receiver.set_output("hdmi");
        assert_eq!((setup.receiver.output_target(), setup.receiver.output_target_label()), ("hdmi", "HDMI"));
        assert!(setup.world.borrow().timers.contains(&Timer::Commit));
        assert_eq!(setup.receiver.config.read_sink_target().0, "");
        setup.receiver.on_timer(Timer::Commit);
        assert_eq!(setup.receiver.config.read_sink_target(), ("hdmi".to_string(), "HDMI".to_string()));
        assert_eq!(setup.world.borrow().pw_metadata, [["71", "target.object", "hdmi"]]);
        assert!(!setup.world.borrow().timers.contains(&Timer::Commit));

        // The sender is still connected, so the receiver isn't restarted yet.
        assert!(setup.receiver.restart_pending);
        assert_eq!(setup.world.borrow().calls, Vec::<String>::new());
        go_online(&setup.world, "Stopped", false);
        setup.receiver.on_timer(Timer::Refresh);
        assert_eq!(setup.world.borrow().calls, ["TryRestartUnit(shairport-sync.service, replace)"]);
        assert_eq!(setup.world.borrow().poll_unit_soon, 1);
        // Only once for that run of the receiver.
        setup.receiver.on_timer(Timer::Unit);
        assert_eq!(setup.world.borrow().calls.len(), 1);

        // The run that follows has the setting, and its stream is left alone.
        run_unit(&setup.world, 2, 5000.0);
        setup.receiver.on_timer(Timer::Unit);
        assert!(!setup.receiver.restart_pending);
        setup.world.borrow_mut().wall = 5020.0;
        look(&mut setup, &pw_dump("usb", true, false, None));
        assert_eq!(setup.world.borrow().pw_metadata.len(), 1);
        assert_eq!(setup.world.borrow().calls.len(), 1);
    }

    #[test]
    fn a_new_run_is_left_to_settle_before_a_restart() {
        let mut setup = setup("settle", "");
        run_unit(&setup.world, 1, 4995.0);
        go_online(&setup.world, "Stopped", false);
        setup.receiver.start();
        setup.receiver.set_output("usb");
        setup.receiver.commit_output();
        assert!(setup.receiver.restart_pending);
        assert_eq!(setup.world.borrow().calls, Vec::<String>::new());
        setup.world.borrow_mut().wall = 5006.0;
        setup.receiver.on_timer(Timer::Unit);
        assert_eq!(setup.world.borrow().calls, ["TryRestartUnit(shairport-sync.service, replace)"]);
    }

    #[test]
    fn quitting_restarts_without_waiting() {
        let mut setup = setup("quit", "");
        run_unit(&setup.world, 1, 4999.0);
        go_online(&setup.world, "Stopped", false);
        setup.receiver.start();
        setup.receiver.set_output("usb");
        setup.receiver.quit();
        assert_eq!(setup.receiver.config.read_sink_target().0, "usb");
        assert_eq!(setup.world.borrow().calls, ["TryRestartUnit(shairport-sync.service, replace)"]);
    }

    #[test]
    fn a_stream_stays_on_the_device_whose_volume_the_receiver_sets() {
        let mut setup = setup("volume", "\tmixer_type = \"sink\";\n");
        assert!(setup.receiver.device_volume());
        setup.receiver.start();
        look(&mut setup, &pw_dump("usb", false, false, None));
        // The app sees the receiver start, and looks at PipeWire once it has settled.
        run_unit(&setup.world, 1, 5001.0);
        go_online(&setup.world, "Playing", true);
        setup.world.borrow_mut().wall = 5002.0;
        setup.receiver.on_timer(Timer::Unit);
        setup.world.borrow_mut().wall = 5010.0;
        look(&mut setup, &pw_dump("usb", true, true, None));
        assert_eq!(setup.receiver.device_volume_warning(), "");
        assert_eq!(setup.world.borrow().pw_metadata, Vec::<Vec<String>>::new());
        setup.receiver.take_events();

        // The default output changes: the stream is put back, once.
        setup.world.borrow_mut().wall = 5013.0;
        look(&mut setup, &pw_dump("hdmi", true, true, None));
        assert_eq!(setup.world.borrow().pw_metadata, [["71", "target.object", "usb"]]);
        let notices: Vec<_> = setup.receiver.take_events().into_iter().filter(|e| matches!(e, Event::Notice(_))).collect();
        assert_eq!(
            notices,
            [Event::Notice(
                "AirPlay keeps playing on USB Audio until this session ends, because the sender's volume control \
                 sets the volume of that device."
                    .to_string()
            )]
        );
        setup.world.borrow_mut().wall = 5016.0;
        look(&mut setup, &pw_dump("hdmi", true, true, None));
        assert_eq!(setup.world.borrow().pw_metadata.len(), 1);
        // Picking another output says why nothing moves yet.
        setup.receiver.set_output("hdmi");
        setup.receiver.commit_output();
        assert_eq!(setup.world.borrow().pw_metadata.len(), 1);
        assert!(setup.receiver.take_events().contains(&Event::Notice(
            "AirPlay switches to HDMI when this session ends. Until then it stays on USB Audio, because the \
             sender's volume control sets the volume of that device."
                .to_string()
        )));
        assert_eq!(setup.world.borrow().calls, Vec::<String>::new());
    }

    #[test]
    fn warns_when_device_volume_fell_back() {
        let mut setup = setup("fallback", "\tmixer_type = \"sink\";\n\tsink_target = \"bt\"; // AirPlay Receiver: Headphones\n");
        setup.receiver.start();
        look(&mut setup, &pw_dump("usb", false, false, None));
        run_unit(&setup.world, 1, 5001.0);
        go_online(&setup.world, "Stopped", false);
        setup.world.borrow_mut().wall = 5002.0;
        setup.receiver.on_timer(Timer::Unit);
        setup.world.borrow_mut().wall = 5004.0;
        look(&mut setup, &pw_dump("usb", false, false, None));
        assert_eq!(setup.receiver.device_volume_warning(), "");
        setup.world.borrow_mut().wall = 5020.0;
        look(&mut setup, &pw_dump("usb", false, false, None));
        assert_eq!(
            setup.receiver.device_volume_warning(),
            "Use Device Volume isn't in effect: Headphones isn't connected, so the sender's volume control turns \
             the audio down digitally. This app restarts the receiver to use the device's volume once it is \
             connected and nothing is playing."
        );
        assert_eq!(setup.world.borrow().calls, Vec::<String>::new());
        // What it started with is remembered for the app's next run.
        let record: Json = serde_json::from_slice(&fs::read(&setup.receiver.state_file).unwrap()).unwrap();
        assert_eq!(
            record,
            serde_json::json!({"process": "01".repeat(16), "target": "bt", "device_volume": true,
                               "default_sink": "usb", "target_present": false})
        );
    }

    #[test]
    fn shows_a_failing_unit_and_resets_it_on_start() {
        let mut setup = setup("failing", "");
        setup.world.borrow_mut().props.insert(
            SYSTEMD_UNIT,
            props(&[("ActiveState", s("activating")), ("SubState", s("auto-restart"))]),
        );
        setup.receiver.start();
        assert_eq!((setup.receiver.unit_state(), setup.receiver.failed_unit()), ("failing", "shairport-sync"));
        setup.world.borrow_mut().props.insert(SYSTEMD_UNIT, props(&[("ActiveState", s("inactive"))]));
        setup.world.borrow_mut().props.insert("timing", props(&[("ActiveState", s("failed"))]));
        setup.receiver.on_timer(Timer::Unit);
        assert_eq!((setup.receiver.unit_state(), setup.receiver.failed_unit()), ("failed", "nqptp"));
        setup.receiver.start_receiver();
        assert_eq!(
            setup.world.borrow().calls,
            [
                "ResetFailedUnit(shairport-sync.service)",
                "ResetFailedUnit(nqptp.service)",
                "StartUnit(shairport-sync.service, replace)"
            ]
        );
        setup.receiver.stop_receiver();
        assert_eq!(
            setup.world.borrow().calls[3..],
            ["StopUnit(shairport-sync.service, replace)", "StopUnit(nqptp.service, replace)"]
        );
    }
}
