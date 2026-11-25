#include "shairport-sync-gui/cpp/shim.h"

#include "shairport-sync-gui/src/object.cxxqt.h"
#include "shairport-sync-gui/src/shim.cxx.h"

#include <QtGui/QDesktopServices>
#include <QtGui/QIcon>
#include <QtGui/QWindow>
#include <QtQml/QQmlApplicationEngine>
#include <QtQml/QQmlError>
#include <QtWidgets/QApplication>
#include <QtWidgets/QMessageBox>

#include <memory>
#include <string>
#include <vector>

namespace shim {

namespace {
// argc and argv must outlive the QApplication.
std::vector<std::string> g_arg_storage;
std::vector<char*> g_argv;
int g_argc = 0;
std::unique_ptr<QApplication> g_app;
std::unique_ptr<QQmlApplicationEngine> g_engine;
ReceiverObject* g_receiver = nullptr;
QStringList g_warnings;

QWindow* root_window()
{
    if (!g_engine || g_engine->rootObjects().isEmpty()) {
        return nullptr;
    }
    return qobject_cast<QWindow*>(g_engine->rootObjects().constFirst());
}
} // namespace

void app_init(rust::Vec<rust::String> args)
{
    for (const auto& arg : args) {
        g_arg_storage.emplace_back(std::string(arg));
    }
    for (auto& arg : g_arg_storage) {
        g_argv.push_back(arg.data());
    }
    g_argc = static_cast<int>(g_argv.size());
    g_argv.push_back(nullptr);

    g_app = std::make_unique<QApplication>(g_argc, g_argv.data());
    QCoreApplication::setApplicationName(QStringLiteral("shairport-sync-gui"));
    QCoreApplication::setOrganizationName(QStringLiteral("shairport-sync"));
    // Matches shairport-sync's MPRIS DesktopEntry, so Plasma ties the two together.
    QGuiApplication::setDesktopFileName(QStringLiteral("shairport-sync"));
    QGuiApplication::setWindowIcon(
        QIcon::fromTheme(QStringLiteral("shairport-sync"), QIcon(QStringLiteral(":/shairport-sync.svg"))));

    QObject::connect(g_app.get(), &QCoreApplication::aboutToQuit, g_app.get(), []() { shim::on_about_to_quit(); });
}

void create_engine()
{
    g_engine = std::make_unique<QQmlApplicationEngine>();
    QObject::connect(g_engine.get(), &QQmlEngine::warnings, g_engine.get(), [](const QList<QQmlError>& warnings) {
        for (const auto& warning : warnings) {
            g_warnings.append(warning.toString());
        }
    });
}

QStringList import_paths()
{
    return g_engine ? g_engine->importPathList() : QStringList();
}

ReceiverObject* create_receiver()
{
    g_receiver = new ReceiverObject(g_engine.get());
    return g_receiver;
}

ReceiverObject* receiver()
{
    return g_receiver;
}

bool load_qml(const QUrl& url)
{
    g_engine->setInitialProperties({{QStringLiteral("receiver"), QVariant::fromValue(g_receiver)}});
    g_engine->load(url);
    return !g_engine->rootObjects().isEmpty();
}

QStringList qml_warnings()
{
    return g_warnings;
}

void show_error(const QString& title, const QString& text)
{
    QMessageBox::critical(nullptr, title, text);
}

bool open_url(const QUrl& url)
{
    return QDesktopServices::openUrl(url);
}

void activate_window(const QString& token)
{
    QWindow* window = root_window();
    if (!window) {
        return;
    }
    if (!token.isEmpty()) {
        // On Wayland a window may only take the focus with the token the
        // launcher gave the new launch.
        qputenv("XDG_ACTIVATION_TOKEN", token.toUtf8());
    }
    if (window->visibility() == QWindow::Minimized) {
        window->showNormal();
    } else {
        window->show();
    }
    window->raise();
    window->requestActivate();
}

int app_exec()
{
    return QApplication::exec();
}

void app_shutdown()
{
    g_engine.reset(); // with the receiver, its child
    g_receiver = nullptr;
    g_app.reset();
}

} // namespace shim
