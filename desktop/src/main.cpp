// torrentplayer — stream and download torrents on the desktop, with the
// Torrent Player engine from the Android app.

#include <QApplication>
#include <QDir>
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
#include <QUrl>
#include <cstdio>

#include "addsession.h"
#include "autostart.h"
#include "battery.h"
#include "connections.h"
#include "engine.h"
#include "format.h"
#include "library.h"
#include "mpvitem.h"
#include "pipwindow.h"
#include "playback.h"
#include "portalfilepicker.h"
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
    return QString("torrentplayer-%1").arg(qEnvironmentVariable("USER", "user"));
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
    QApplication app(argc, argv);
    app.setApplicationName("torrentplayer");
    app.setApplicationVersion(QLatin1String(APP_VERSION));
    app.setDesktopFileName("torrentplayer");
    app.setWindowIcon(QIcon::fromTheme("torrentplayer"));
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

    const QString data = QStandardPaths::writableLocation(QStandardPaths::AppDataLocation);
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
    PortalFilePicker picker;

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
    PipWindow pipWindow;
    qml.rootContext()->setContextProperty("pipWindow", &pipWindow);
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
