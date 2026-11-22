// torrentplayer — stream and download torrents on the desktop, with the
// Torrent Player engine from the Android app.

#include <QDir>
#include <QGuiApplication>
#include <QIcon>
#include <QLocalServer>
#include <QLocalSocket>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickStyle>
#include <QQuickWindow>
#include <QSortFilterProxyModel>
#include <QStandardPaths>
#include <QUrl>
#include <cstdio>

#include "addsession.h"
#include "engine.h"
#include "format.h"
#include "library.h"
#include "portalfilepicker.h"
#include "settings.h"
#include "subtitles.h"
#include "theme.h"
#include "translator.h"

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
    QGuiApplication app(argc, argv);
    app.setApplicationName("torrentplayer");
    app.setApplicationVersion(QLatin1String(APP_VERSION));
    app.setDesktopFileName("torrentplayer");
    app.setWindowIcon(QIcon::fromTheme("torrentplayer"));

    QString link;
    for (const QString &argument : app.arguments().mid(1)) {
        if (argument == "--help" || argument == "-h") {
            std::printf("Usage: torrentplayer [magnet link | torrent URL | file.torrent]\n");
            return 0;
        }
        if (argument == "--version") {
            std::printf("torrentplayer %s\n", APP_VERSION);
            return 0;
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
    Library library(&engine, data, downloads + "/Torrent Player");
    AddSession adder(&engine, &library);
    Subtitles subtitles;
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
    qml.load(QUrl("qrc:/Main.qml"));
    if (qml.rootObjects().isEmpty())
        return 1;
    auto *window = qobject_cast<QQuickWindow *>(qml.rootObjects().first());

    QObject::connect(&server, &QLocalServer::newConnection, &app, [&] {
        while (QLocalSocket *socket = server.nextPendingConnection()) {
            QObject::connect(socket, &QLocalSocket::disconnected, socket, &QObject::deleteLater);
            QObject::connect(socket, &QLocalSocket::readyRead, socket, [socket, &adder, window] {
                while (socket->canReadLine()) {
                    const QString incoming = QString::fromUtf8(socket->readLine()).trimmed();
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

    // The engine stops with the app; quitting closes its files cleanly.
    QObject::connect(&app, &QGuiApplication::aboutToQuit, &engine, &Engine::stop);
    engine.start();
    if (!link.isEmpty())
        adder.start(link);

    return app.exec();
}
