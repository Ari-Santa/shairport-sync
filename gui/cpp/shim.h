// What the app needs of Qt that cxx-qt-lib doesn't wrap: the QApplication,
// the QML engine and its window.
#pragma once

#include <QtCore/QString>
#include <QtCore/QStringList>
#include <QtCore/QUrl>

#include "rust/cxx.h"

class ReceiverObject;

namespace shim {

void app_init(rust::Vec<rust::String> args);
void create_engine();
QStringList import_paths();
ReceiverObject* create_receiver();
ReceiverObject* receiver();
bool load_qml(const QUrl& url);
QStringList qml_warnings();
void show_error(const QString& title, const QString& text);
bool open_url(const QUrl& url);
void activate_window(const QString& token);
int app_exec();
void app_shutdown();

} // namespace shim
