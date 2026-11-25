//! AirPlay Receiver -- a now-playing window for Shairport Sync.
//!
//! Reads the MPRIS and native D-Bus interfaces that shairport-sync publishes
//! on the session bus (set dbus_service_bus and mpris_service_bus to
//! "session") and drives a Kirigami UI. It also starts and stops the
//! receiver's systemd --user units (shairport-sync.service and
//! nqptp.service).

mod config;
mod dbus;
mod host;
mod object;
mod receiver;
mod shim;

use std::path::Path;
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::mpsc;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QUrl};

use object::qobject::ReceiverObject;

fn main() -> ExitCode {
    // One window: a second launch hands over to the first and exits.
    let connection = dbus::connect();
    let (launches, later_launches) = mpsc::channel();
    if let Some(connection) = &connection {
        if !dbus::claim_single_instance(connection, launches) {
            return ExitCode::SUCCESS;
        }
    }

    shim::ffi::app_init(std::env::args().collect());
    shim::ffi::create_engine();
    // Use Plasma's own Qt Quick Controls style, where it is installed, so the
    // window matches the desktop.
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none()
        && shim::ffi::import_paths()
            .iter()
            .any(|path| Path::new(&path.to_string()).join("org/kde/desktop/qmldir").is_file())
    {
        // SAFETY: no other thread reads or changes the environment yet: the
        // threads there are (the bus connection's) don't, and the receiver's
        // start below.
        unsafe { std::env::set_var("QT_QUICK_CONTROLS_STYLE", "org.kde.desktop") };
    }

    // SAFETY: the object lives on this thread, Qt's, until `app_shutdown`,
    // and this is the only reference to it for as long as it is used here.
    let mut receiver: Pin<&mut ReceiverObject> = unsafe {
        Pin::new_unchecked(shim::ffi::create_receiver().as_mut().expect("the receiver object was just created"))
    };
    receiver.as_mut().start(connection);
    let qt_thread = receiver.qt_thread();

    if !shim::ffi::load_qml(&QUrl::from("qrc:/qml/Main.qml")) {
        // Started from the launcher, nobody sees what went to stderr.
        let warnings: Vec<String> = shim::ffi::qml_warnings().iter().take(10).map(QString::to_string).collect();
        let text = format!(
            "AirPlay Receiver couldn't open its window. A Qt Quick module it needs may not be installed.\n\n{}",
            warnings.join("\n")
        );
        shim::ffi::show_error(&QString::from("AirPlay Receiver"), &QString::from(&text));
        shim::ffi::app_shutdown();
        return ExitCode::FAILURE;
    }

    // Later launches bring the window forward.
    std::thread::spawn(move || {
        for token in later_launches {
            let activate = move |_: Pin<&mut ReceiverObject>| shim::ffi::activate_window(&QString::from(&token));
            if qt_thread.queue(activate).is_err() {
                break;
            }
        }
    });

    let code = shim::ffi::app_exec();
    shim::ffi::app_shutdown();
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
