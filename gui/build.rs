use cxx_qt_build::CxxQtBuilder;

fn main() {
    CxxQtBuilder::new()
        .file("src/object.rs")
        .file("src/shim.rs")
        // Core is implied.
        .qt_module("Gui")
        .qt_module("Widgets")
        .qt_module("Qml")
        .qt_module("Quick")
        .cpp_file("cpp/shim.cpp")
        // qrc:/qml/Main.qml and qrc:/shairport-sync.svg
        .qrc("resources.qrc")
        .build();
    println!("cargo::rerun-if-changed=cpp/shim.h");
    println!("cargo::rerun-if-changed=qml/Main.qml");
    println!("cargo::rerun-if-changed=shairport-sync.svg");
}
