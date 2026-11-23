// torrentplayer — stream and download torrents on the desktop, with the
// Torrent Player engine from the Android app.

#include <QApplication>
#include <QDir>
#include <QFileOpenEvent>
#include <QIcon>
#include <QLocalServer>
#include <QLocalSocket>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQmlEngine>
#include <QQuickStyle>
#include <QQuickWindow>
#include <QSGRendererInterface>
#include <QSortFilterProxyModel>
#include <QStandardPaths>
#include <QSurfaceFormat>
#include <QUrl>
#include <cstdio>
#include <functional>

#include "addsession.h"
#include "autostart.h"
#include "battery.h"
#include "connections.h"
#include "engine.h"
#include "format.h"
#include "library.h"
#include "mpvitem.h"
#include "playback.h"
#include "nativefilepicker.h"
#include "platform.h"
#if TORRENTPLAYER_FREEDESKTOP
#include "portalfilepicker.h"
#endif
#include "settings.h"
#include "subtitles.h"
#include "theme.h"
#include "translator.h"
#include "tray.h"
#include "worldmap.h"

// Written by desktop/bin/build from Android versionName into build/app_version.h.
#if __has_include("app_version.h")
#include "app_version.h"
#endif
#ifndef APP_VERSION
#define APP_VERSION "0.0.0"
#endif

static QString serverName() {
    return Platform::instanceServerName(qEnvironmentVariable("USER", qEnvironmentVariable("USERNAME")));
}

// macOS hands magnet links and .torrent files opened from Finder or the browser to
// the running app as QFileOpenEvent, not as arguments.
class FileOpenFilter : public QObject {
public:
    explicit FileOpenFilter(std::function<void(const QString &)> open, QObject *parent = nullptr)
        : QObject(parent), m_open(std::move(open)) {}

protected:
    bool eventFilter(QObject *watched, QEvent *event) override {
        if (event->type() == QEvent::FileOpen) {
            auto *open = static_cast<QFileOpenEvent *>(event);
            const QString link = open->url().isLocalFile() ? open->file() : open->url().toString();
            if (!link.isEmpty())
                m_open(link);
            return true;
        }
        return QObject::eventFilter(watched, event);
    }

private:
    std::function<void(const QString &)> m_open;
};

// Graphics setup that must happen before the application object exists. mpv draws
// through OpenGL: macOS only offers a modern context as a 3.2+ core profile, and
// Windows must not fall back to ANGLE or software GL.
static void prepareOpenGL() {
#if defined(Q_OS_MACOS)
    QSurfaceFormat format = QSurfaceFormat::defaultFormat();
    format.setRenderableType(QSurfaceFormat::OpenGL);
    format.setVersion(3, 2);
    format.setProfile(QSurfaceFormat::CoreProfile);
    QSurfaceFormat::setDefaultFormat(format);
#elif defined(Q_OS_WIN)
    QCoreApplication::setAttribute(Qt::AA_UseDesktopOpenGL);
#endif
}

// One window and one engine per user: a second launch (a magnet clicked in
// the browser, a .torrent opened from the file manager) hands its link to the
// running app and exits.
static bool forwardToRunningApp(const QString &link) {
    QLocalSocket socket;
    socket.connectToServer(serverName());
    if (!socket.waitForConnected(500))
        return false;
    socket.write((link.isEmpty() ? QString("\n") : link + "\n").toUtf8());
    socket.waitForBytesWritten(1000);
    socket.disconnectFromServer();
    return true;
}

int main(int argc, char *argv[]) {
    prepareOpenGL();
    QApplication app(argc, argv);
    app.setApplicationName("torrentplayer");
    app.setApplicationVersion(QLatin1String(APP_VERSION));
    app.setDesktopFileName("torrentplayer");
    // The theme's icon on Linux; the built-in one where there is none (Windows, macOS).
    app.setWindowIcon(QIcon::fromTheme("torrentplayer", QIcon(":/icons/torrentplayer.png")));
    app.setQuitOnLastWindowClosed(false);

    QString link;
    bool startBackground = false;
    for (const QString &argument : app.arguments().mid(1)) {
        if (argument == "--help" || argument == "-h") {
            std::printf("Usage: torrentplayer [--background] [magnet link | torrent URL | file.torrent]\n");
            return 0;
        }
        if (argument == "--version") {
            std::printf("torrentplayer %s\n", APP_VERSION);
            return 0;
        }
        if (argument == "--background") {
            startBackground = true;
            continue;
        }
        if (!argument.startsWith("-"))
            link = argument.startsWith("file:") ? QUrl(argument).toLocalFile() : argument;
    }
    if (!link.isEmpty() && !link.startsWith("magnet:") && !link.contains("://"))
        link = QDir::current().absoluteFilePath(link);
    if (forwardToRunningApp(link))
        return 0;

    QLocalServer::removeServer(serverName());
    QLocalServer server;
    server.listen(serverName());

    QQuickStyle::setStyle("Material");
    qmlRegisterType<Connections>("Webtor", 1, 0, "TorrentConnections");
    qmlRegisterType<WorldMap>("Webtor", 1, 0, "WorldMap");
    // The player draws through libmpv's OpenGL render API.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    qmlRegisterType<MpvPlayer>("TorrentPlayer", 1, 0, "MpvPlayer");
    qmlRegisterType<PlaybackControl>("TorrentPlayer", 1, 0, "PlaybackControl");

    Translator translator;
    app.installTranslator(&translator);

    // ~/.local/share/torrentplayer on Linux, %LOCALAPPDATA%\torrentplayer on Windows (not
    // the roaming profile: the engine keeps big part files here), and
    // ~/Library/Application Support/torrentplayer on macOS.
    const QString data = QStandardPaths::writableLocation(QStandardPaths::AppLocalDataLocation);
    QString downloads = QStandardPaths::writableLocation(QStandardPaths::DownloadLocation);
    if (downloads.isEmpty())
        downloads = QDir::homePath() + "/Downloads";

    Theme theme;
    Format format;
    Engine engine(data + "/engine");
    // User settings from shared/policy.json defaults; the engine hears them on every start.
    Settings settings(Settings::defaultPath());
    EngineSettings engineSettings(&engine, &settings);
    DeviceStatus deviceStatus(&settings, &engine);
    QObject::connect(&deviceStatus, &DeviceStatus::changed, &engineSettings, [&] {
        engineSettings.setConditions(deviceStatus.conditions());
    });
    engineSettings.setConditions(deviceStatus.conditions());

    const QString saveFolder =
        settings.downloadFolder().isEmpty() ? downloads + "/Torrent Player" : settings.downloadFolder();
    Library library(&engine, data, saveFolder);
    library.setSettings(&settings);
    AddSession adder(&engine, &library);
    Subtitles subtitles;
    Autostart autostart;
    // The home screen's search filters the library by name.
    QSortFilterProxyModel libraryView;
    libraryView.setSourceModel(&library);
    libraryView.setFilterRole(Library::NameRole);
    libraryView.setFilterCaseSensitivity(Qt::CaseInsensitive);
#if TORRENTPLAYER_FREEDESKTOP
    PortalFilePicker picker;
#else
    NativeFilePicker picker;
#endif

    QQmlApplicationEngine qml;
    QObject::connect(&translator, &Translator::changed, &qml, &QQmlEngine::retranslate);
    qml.rootContext()->setContextProperty("translator", &translator);
    qml.rootContext()->setContextProperty("theme", &theme);
    qml.rootContext()->setContextProperty("fmt", &format);
    qml.rootContext()->setContextProperty("engine", &engine);
    qml.rootContext()->setContextProperty("settings", &settings);
    qml.rootContext()->setContextProperty("library", &library);
    qml.rootContext()->setContextProperty("libraryView", &libraryView);
    qml.rootContext()->setContextProperty("adder", &adder);
    qml.rootContext()->setContextProperty("subtitles", &subtitles);
    qml.rootContext()->setContextProperty("picker", &picker);
    Tray tray(&library);
    qml.rootContext()->setContextProperty("deviceStatus", &deviceStatus);
    qml.rootContext()->setContextProperty("autostart", &autostart);
    qml.rootContext()->setContextProperty("tray", &tray);
    qml.load(QUrl("qrc:/Main.qml"));
    if (qml.rootObjects().isEmpty())
        return 1;
    auto *window = qobject_cast<QQuickWindow *>(qml.rootObjects().first());
    tray.setWindow(window);

    if (startBackground && window) {
        window->setVisible(false);
        tray.setBackgroundHintShown(true);
    }

    QObject::connect(&server, &QLocalServer::newConnection, &app, [&] {
        while (QLocalSocket *socket = server.nextPendingConnection()) {
            QObject::connect(socket, &QLocalSocket::disconnected, socket, &QObject::deleteLater);
            QObject::connect(socket, &QLocalSocket::readyRead, socket, [socket, &adder, &tray, window] {
                while (socket->canReadLine()) {
                    const QString incoming = QString::fromUtf8(socket->readLine()).trimmed();
                    tray.showWindow();
                    if (window) {
                        window->show();
                        window->raise();
                        window->requestActivate();
                    }
                    if (!incoming.isEmpty())
                        adder.start(incoming);
                }
            });
        }
    });

#ifdef Q_OS_MACOS
    FileOpenFilter fileOpen([&](const QString &incoming) {
        tray.showWindow();
        adder.start(incoming);
    });
    app.installEventFilter(&fileOpen);
#endif

    // Keep active-transfer count for Auto RAM sizing.
    QObject::connect(&library, &Library::revisionChanged, &settings, [&] {
        settings.setActiveTransfers(std::max(1, library.activeCount()));
    });
    settings.setActiveTransfers(std::max(1, library.activeCount()));

    // The engine stops with the app; quitting closes its files cleanly.
    QObject::connect(&app, &QGuiApplication::aboutToQuit, &engine, &Engine::stop);
    engine.start();
    if (!link.isEmpty())
        adder.start(link);

    return app.exec();
}
