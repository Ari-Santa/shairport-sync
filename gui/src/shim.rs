//! What the app needs of Qt that cxx-qt-lib doesn't wrap, in cpp/shim.cpp:
//! the QApplication, the QML engine and its window.

use std::pin::Pin;

#[cxx::bridge(namespace = "shim")]
pub mod ffi {
    #[namespace = ""]
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qurl.h");
        type QUrl = cxx_qt_lib::QUrl;
        type ReceiverObject = crate::object::qobject::ReceiverObject;
    }

    unsafe extern "C++" {
        include!("shairport-sync-gui/cpp/shim.h");

        /// Create the QApplication and set its names and window icon.
        fn app_init(args: Vec<String>);
        fn create_engine();
        fn import_paths() -> QStringList;
        /// Create the object the window gets as its `receiver`; the engine owns it.
        fn create_receiver() -> *mut ReceiverObject;
        /// That object, or null before `create_receiver` and after `app_shutdown`.
        fn receiver() -> *mut ReceiverObject;
        /// Load the window; true if it opened.
        fn load_qml(url: &QUrl) -> bool;
        fn qml_warnings() -> QStringList;
        fn show_error(title: &QString, text: &QString);
        fn open_url(url: &QUrl) -> bool;
        /// Bring the window forward, with the activation token of the launch
        /// that asks for it, if it has one.
        fn activate_window(token: &QString);
        fn app_exec() -> i32;
        fn app_shutdown();
    }

    extern "Rust" {
        fn on_about_to_quit();
    }
}

/// QCoreApplication::aboutToQuit.
fn on_about_to_quit() {
    // SAFETY: called by Qt on its own thread, where the object lives until
    // `app_shutdown`, and nothing else holds a reference to it meanwhile.
    if let Some(object) = unsafe { ffi::receiver().as_mut() } {
        unsafe { Pin::new_unchecked(object) }.quit();
    }
}
