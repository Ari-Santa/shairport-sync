//! The `receiver` object the window gets: the receiver's state as QML
//! properties, what it reports as signals, and what the window can ask of it.

use std::path::PathBuf;
use std::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QMap, QMapPair_QString_QVariant, QString, QVariant};

use crate::config::{home_dir, Config};
use crate::dbus::{self, SessionBus, Signal};
use crate::host::QtHost;
use crate::receiver::{Event, Receiver};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qlist.h");
        type QList_QVariant = cxx_qt_lib::QList<cxx_qt_lib::QVariant>;
    }

    // Rust's snake_case names are camelCase in QML.
    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qproperty(bool, online, READ = get_online, NOTIFY = state_changed)]
        #[qproperty(QString, receiver_name, READ = get_receiver_name, NOTIFY = state_changed)]
        #[qproperty(QString, version, READ = get_version, NOTIFY = state_changed)]
        #[qproperty(QString, protocol, READ = get_protocol, NOTIFY = state_changed)]
        #[qproperty(bool, active, READ = get_active, NOTIFY = state_changed)]
        #[qproperty(QString, client_name, READ = get_client_name, NOTIFY = state_changed)]
        #[qproperty(QString, client_address, READ = get_client_address, NOTIFY = state_changed)]
        #[qproperty(QString, playback_status, READ = get_playback_status, NOTIFY = state_changed)]
        #[qproperty(QString, title, READ = get_title, NOTIFY = state_changed)]
        #[qproperty(QString, artist, READ = get_artist, NOTIFY = state_changed)]
        #[qproperty(QString, album, READ = get_album, NOTIFY = state_changed)]
        #[qproperty(QString, art_url, READ = get_art_url, NOTIFY = state_changed)]
        #[qproperty(f64, duration, READ = get_duration, NOTIFY = state_changed)]
        #[qproperty(f64, volume, READ = get_volume, NOTIFY = state_changed)]
        #[qproperty(QString, source_format, READ = get_source_format, NOTIFY = state_changed)]
        #[qproperty(QString, output_format, READ = get_output_format, NOTIFY = state_changed)]
        #[qproperty(QString, unit_state, READ = get_unit_state, NOTIFY = state_changed)]
        #[qproperty(QString, failed_unit, READ = get_failed_unit, NOTIFY = state_changed)]
        #[qproperty(bool, in_session, READ = get_in_session, NOTIFY = state_changed)]
        #[qproperty(f64, position, READ = get_position, NOTIFY = position_changed)]
        #[qproperty(QString, config_file, READ = get_config_file, CONSTANT)]
        #[qproperty(bool, output_switching_available, READ = get_output_switching_available, NOTIFY = outputs_changed)]
        #[qproperty(QList_QVariant, outputs, READ = get_outputs, NOTIFY = outputs_changed)]
        #[qproperty(QString, default_output, READ = get_default_output, NOTIFY = outputs_changed)]
        #[qproperty(QString, output_target, READ = get_output_target, NOTIFY = outputs_changed)]
        #[qproperty(QString, output_target_label, READ = get_output_target_label, NOTIFY = outputs_changed)]
        #[qproperty(bool, device_volume_available, READ = get_device_volume_available, NOTIFY = device_volume_changed)]
        #[qproperty(bool, device_volume, READ = get_device_volume, NOTIFY = device_volume_changed)]
        #[qproperty(QString, device_volume_warning, READ = get_device_volume_warning, NOTIFY = device_volume_changed)]
        type ReceiverObject = super::ReceiverObjectRust;

        #[qsignal]
        fn state_changed(self: Pin<&mut ReceiverObject>);
        #[qsignal]
        fn position_changed(self: Pin<&mut ReceiverObject>);
        #[qsignal]
        fn outputs_changed(self: Pin<&mut ReceiverObject>);
        #[qsignal]
        fn device_volume_changed(self: Pin<&mut ReceiverObject>);
        #[qsignal]
        fn error_occurred(self: Pin<&mut ReceiverObject>, message: QString);
        #[qsignal]
        fn notice_occurred(self: Pin<&mut ReceiverObject>, message: QString);

        fn get_online(self: &ReceiverObject) -> bool;
        fn get_receiver_name(self: &ReceiverObject) -> QString;
        fn get_version(self: &ReceiverObject) -> QString;
        fn get_protocol(self: &ReceiverObject) -> QString;
        fn get_active(self: &ReceiverObject) -> bool;
        fn get_client_name(self: &ReceiverObject) -> QString;
        fn get_client_address(self: &ReceiverObject) -> QString;
        fn get_playback_status(self: &ReceiverObject) -> QString;
        fn get_title(self: &ReceiverObject) -> QString;
        fn get_artist(self: &ReceiverObject) -> QString;
        fn get_album(self: &ReceiverObject) -> QString;
        fn get_art_url(self: &ReceiverObject) -> QString;
        fn get_duration(self: &ReceiverObject) -> f64;
        fn get_volume(self: &ReceiverObject) -> f64;
        fn get_source_format(self: &ReceiverObject) -> QString;
        fn get_output_format(self: &ReceiverObject) -> QString;
        fn get_unit_state(self: &ReceiverObject) -> QString;
        fn get_failed_unit(self: &ReceiverObject) -> QString;
        fn get_in_session(self: &ReceiverObject) -> bool;
        fn get_position(self: &ReceiverObject) -> f64;
        fn get_config_file(self: &ReceiverObject) -> QString;
        fn get_output_switching_available(self: &ReceiverObject) -> bool;
        fn get_outputs(self: &ReceiverObject) -> QList_QVariant;
        fn get_default_output(self: &ReceiverObject) -> QString;
        fn get_output_target(self: &ReceiverObject) -> QString;
        fn get_output_target_label(self: &ReceiverObject) -> QString;
        fn get_device_volume_available(self: &ReceiverObject) -> bool;
        fn get_device_volume(self: &ReceiverObject) -> bool;
        fn get_device_volume_warning(self: &ReceiverObject) -> QString;

        #[qinvokable]
        fn refresh(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn refresh_outputs(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn set_output(self: Pin<&mut ReceiverObject>, name: &QString);
        #[qinvokable]
        fn commit_output(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn set_device_volume(self: Pin<&mut ReceiverObject>, enabled: bool);
        #[qinvokable]
        fn play_pause(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn next(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn previous(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn set_volume(self: Pin<&mut ReceiverObject>, value: f64);
        #[qinvokable]
        fn disconnect_sender(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn start_receiver(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn stop_receiver(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn restart_receiver(self: Pin<&mut ReceiverObject>);
        #[qinvokable]
        fn open_config(self: Pin<&mut ReceiverObject>);
    }

    impl cxx_qt::Threading for ReceiverObject {}
}

/// Empty until `ReceiverObject::start`; the window isn't loaded before that.
#[derive(Default)]
pub struct ReceiverObjectRust {
    receiver: Option<Receiver>,
}

/// What the running receiver process started with is kept here for the app's
/// next run.
fn state_file() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home_dir().join(".local/state"), PathBuf::from)
        .join("shairport-sync-gui/receiver.json")
}

fn qstring(text: &str) -> QString {
    QString::from(text)
}

impl qobject::ReceiverObject {
    /// Connect the object to the receiver, over the session bus if there is
    /// one, and start following it.
    pub fn start(mut self: Pin<&mut Self>, connection: Option<zbus::blocking::Connection>) {
        let qt_thread = self.qt_thread();
        let host = QtHost::new(qt_thread.clone());
        let receiver = Receiver::new(Box::new(SessionBus(connection)), Box::new(host), Config::default(), state_file());
        self.as_mut().rust_mut().receiver = Some(receiver);
        // Watch before the first look, so that a change made during it isn't
        // missed: its signal is queued and handled right after.
        // Without the signals the window still follows the receiver's unit,
        // and its state whenever it is asked to refresh.
        let _ = dbus::watch(move |signal| {
            qt_thread
                .queue(move |object: Pin<&mut Self>| {
                    object.with_receiver(|receiver| match signal {
                        Signal::PropertiesChanged => receiver.on_properties_changed(),
                        Signal::ServiceRegistered => receiver.on_service_registered(),
                        Signal::ServiceUnregistered => receiver.on_service_unregistered(),
                    })
                })
                .is_ok()
        });
        self.as_mut().with_receiver(Receiver::start);
    }

    /// Have the receiver do something, then tell the window what happened.
    pub fn with_receiver(mut self: Pin<&mut Self>, f: impl FnOnce(&mut Receiver)) {
        let events = match self.as_mut().rust_mut().receiver.as_mut() {
            Some(receiver) => {
                f(receiver);
                receiver.take_events()
            }
            None => return,
        };
        // The receiver isn't borrowed any more: what the window does about a
        // signal may well call back into it.
        for event in events {
            match event {
                Event::StateChanged => self.as_mut().state_changed(),
                Event::PositionChanged => self.as_mut().position_changed(),
                Event::OutputsChanged => self.as_mut().outputs_changed(),
                Event::DeviceVolumeChanged => self.as_mut().device_volume_changed(),
                Event::Error(message) => self.as_mut().error_occurred(qstring(&message)),
                Event::Notice(message) => self.as_mut().notice_occurred(qstring(&message)),
            }
        }
    }

    /// The app is about to quit.
    pub fn quit(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::quit);
    }

    fn read<T: Default>(&self, f: impl FnOnce(&Receiver) -> T) -> T {
        self.rust().receiver.as_ref().map(f).unwrap_or_default()
    }

    fn text(&self, f: impl FnOnce(&Receiver) -> &str) -> QString {
        self.rust().receiver.as_ref().map(|receiver| qstring(f(receiver))).unwrap_or_default()
    }

    fn get_online(&self) -> bool {
        self.read(|r| r.state().online)
    }
    fn get_receiver_name(&self) -> QString {
        self.text(|r| &r.state().receiver_name)
    }
    fn get_version(&self) -> QString {
        self.text(|r| &r.state().version)
    }
    fn get_protocol(&self) -> QString {
        self.text(|r| &r.state().protocol)
    }
    fn get_active(&self) -> bool {
        self.read(|r| r.state().active)
    }
    fn get_client_name(&self) -> QString {
        self.text(|r| &r.state().client_name)
    }
    fn get_client_address(&self) -> QString {
        self.text(|r| &r.state().client_address)
    }
    fn get_playback_status(&self) -> QString {
        self.text(|r| &r.state().playback_status)
    }
    fn get_title(&self) -> QString {
        self.text(|r| &r.state().title)
    }
    fn get_artist(&self) -> QString {
        self.text(|r| &r.state().artist)
    }
    fn get_album(&self) -> QString {
        self.text(|r| &r.state().album)
    }
    fn get_art_url(&self) -> QString {
        self.text(|r| &r.state().art_url)
    }
    fn get_duration(&self) -> f64 {
        self.read(|r| r.state().duration)
    }
    fn get_volume(&self) -> f64 {
        self.read(|r| r.state().volume)
    }
    fn get_source_format(&self) -> QString {
        self.text(|r| &r.state().source_format)
    }
    fn get_output_format(&self) -> QString {
        self.text(|r| &r.state().output_format)
    }
    fn get_unit_state(&self) -> QString {
        self.text(Receiver::unit_state)
    }
    fn get_failed_unit(&self) -> QString {
        self.text(Receiver::failed_unit)
    }
    fn get_in_session(&self) -> bool {
        self.read(Receiver::in_session)
    }
    fn get_position(&self) -> f64 {
        self.read(Receiver::position)
    }
    fn get_config_file(&self) -> QString {
        self.read(|receiver| qstring(&receiver.config_file().to_string_lossy()))
    }
    fn get_output_switching_available(&self) -> bool {
        self.read(Receiver::output_switching_available)
    }
    fn get_outputs(&self) -> QList<QVariant> {
        let mut outputs = QList::<QVariant>::default();
        self.read(|receiver| {
            for sink in receiver.outputs() {
                let mut output = QMap::<QMapPair_QString_QVariant>::default();
                output.insert(qstring("name"), QVariant::from(&qstring(&sink.name)));
                output.insert(qstring("description"), QVariant::from(&qstring(&sink.description)));
                outputs.append(QVariant::from(&output));
            }
        });
        outputs
    }
    fn get_default_output(&self) -> QString {
        self.text(Receiver::default_output)
    }
    fn get_output_target(&self) -> QString {
        self.text(Receiver::output_target)
    }
    fn get_output_target_label(&self) -> QString {
        self.text(Receiver::output_target_label)
    }
    fn get_device_volume_available(&self) -> bool {
        self.read(Receiver::device_volume_available)
    }
    fn get_device_volume(&self) -> bool {
        self.read(Receiver::device_volume)
    }
    fn get_device_volume_warning(&self) -> QString {
        self.text(Receiver::device_volume_warning)
    }

    fn refresh(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::refresh);
    }
    fn refresh_outputs(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::refresh_outputs);
    }
    fn set_output(self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        self.with_receiver(|receiver| receiver.set_output(&name));
    }
    fn commit_output(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::commit_output);
    }
    fn set_device_volume(self: Pin<&mut Self>, enabled: bool) {
        self.with_receiver(|receiver| receiver.set_device_volume(enabled));
    }
    fn play_pause(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::play_pause);
    }
    fn next(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::next);
    }
    fn previous(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::previous);
    }
    fn set_volume(self: Pin<&mut Self>, value: f64) {
        self.with_receiver(|receiver| receiver.set_volume(value));
    }
    fn disconnect_sender(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::disconnect_sender);
    }
    fn start_receiver(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::start_receiver);
    }
    fn stop_receiver(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::stop_receiver);
    }
    fn restart_receiver(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::restart_receiver);
    }
    fn open_config(self: Pin<&mut Self>) {
        self.with_receiver(Receiver::open_config);
    }
}
