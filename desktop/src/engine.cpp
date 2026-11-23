#include "engine.h"

#include <QCoreApplication>
#include <QDir>
#include <QEventLoop>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QNetworkReply>
#include <QRegularExpression>
#include <QTcpServer>
#include <QTimer>

#include "platform.h"

#include <QProcessEnvironment>

#ifdef Q_OS_LINUX
#include <csignal>
#include <sys/prctl.h>
#endif

Engine::Engine(const QString &dataDirectory, QObject *parent)
    : QObject(parent), m_dataDirectory(dataDirectory) {
    m_process.setProcessChannelMode(QProcess::MergedChannels);
    // If the app dies without its normal shutdown (killed, crashed, session ended), the
    // engine goes too, instead of running on beside the next start on the same folder.
    // Everywhere: the engine quits when its stdin closes, which happens when this
    // process dies, because only the app holds the pipe's write end. So stdin stays
    // open (QProcess's default) until stop(). On Linux the kernel also signals it.
    QProcessEnvironment environment = QProcessEnvironment::systemEnvironment();
    environment.insert("ENGINE_EXIT_ON_STDIN_EOF", "1");
    m_process.setProcessEnvironment(environment);
#ifdef Q_OS_LINUX
    m_process.setChildProcessModifier([] { ::prctl(PR_SET_PDEATHSIG, SIGTERM); });
#endif
    connect(&m_process, &QProcess::readyRead, this, [this] {
        const QByteArray chunk = m_process.readAll();
        if (m_port > 0)
            return;
        m_output += chunk;
        static const QRegularExpression listening(R"(control listening on http://127\.0\.0\.1:(\d+))");
        const auto match = listening.match(QString::fromUtf8(m_output));
        if (match.hasMatch()) {
            m_port = match.captured(1).toInt();
            m_output.clear();
            m_error.clear();
            emit stateChanged();
            emit started();
        } else if (m_output.size() > 64 * 1024) {
            m_output = m_output.right(8 * 1024);
        }
    });
    connect(&m_process, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
        if (error == QProcess::FailedToStart)
            fail(tr("The torrent engine could not start: %1").arg(m_process.errorString()));
    });
    connect(&m_process, &QProcess::finished, this, [this](int code, QProcess::ExitStatus status) {
        if (m_stopping)
            return;
        const QString tail = QString::fromUtf8(m_output).trimmed().section('\n', -3);
        fail(status == QProcess::CrashExit
                 ? tr("The torrent engine crashed.") + (tail.isEmpty() ? QString() : "\n" + tail)
                 : tr("The torrent engine stopped (exit code %1).").arg(code) + (tail.isEmpty() ? QString() : "\n" + tail));
    });
}

Engine::~Engine() {
    stop();
}

QString Engine::findBinary() {
    const QStringList candidates = Platform::engineCandidates(
        Platform::current(), QCoreApplication::applicationDirPath(), qEnvironmentVariable("TORRENTPLAYER_ENGINE"));
    for (const QString &candidate : candidates)
        if (QFileInfo(candidate).isExecutable())
            return QFileInfo(candidate).absoluteFilePath();
    return {};
}

void Engine::fail(const QString &message) {
    m_port = 0;
    m_error = message;
    emit stateChanged();
}

// The engine looks up peer countries in a geoip-country*.mmdb inside its data folder,
// as on Android, where the app copies the database from its assets. Here the package
// installs it once and the data folder gets a link, so updates replace it in one place.
// Windows gets a copy (links there need admin rights), refreshed when the source changes.
static void linkCountryDatabase(const QString &dataDirectory) {
    const QStringList dirs = Platform::geoipDirectories(
        Platform::current(), QCoreApplication::applicationDirPath(), qEnvironmentVariable("TORRENTPLAYER_GEOIP_DIR"));
    QString source;
    for (const QString &dir : dirs) {
        QStringList found = QDir(dir).entryList({"geoip-country*.mmdb"}, QDir::Files, QDir::Name);
        if (!found.isEmpty()) {
            source = QDir::cleanPath(QDir(dir).absoluteFilePath(found.last()));  // the newest month sorts last
            break;
        }
    }
    if (source.isEmpty())
        return;  // Optional: without it peers just have no country.
    const QDir data(dataDirectory);
    const QString target = data.filePath(QFileInfo(source).fileName());
    for (const QString &name : data.entryList({"geoip-country*.mmdb"}, QDir::Files | QDir::System))
        if (data.filePath(name) != target)
            QFile::remove(data.filePath(name));
    if (Platform::geoipCopiesFile(Platform::current())) {
        const QFileInfo from(source), to(target);
        if (!to.exists() || to.size() != from.size() || to.lastModified() < from.lastModified()) {
            QFile::remove(target);
            QFile::copy(source, target);
        }
    } else if (QFileInfo(target).symLinkTarget() != source) {
        QFile::remove(target);
        QFile::link(source, target);
    }
}

void Engine::start() {
    if (m_process.state() != QProcess::NotRunning)
        return;
    const QString binary = findBinary();
    if (binary.isEmpty()) {
        fail(tr("The torrent engine (%1) is missing. Reinstall Torrent Player.")
                 .arg(Platform::engineBinaryName(Platform::current())));
        return;
    }
    QDir().mkpath(m_dataDirectory);
    linkCountryDatabase(m_dataDirectory);

    // Ask the kernel for a free port; the engine also falls back to a random
    // port when this one is taken, and prints the port it really uses.
    QTcpServer probe;
    probe.listen(QHostAddress::LocalHost, 0);
    const int port = probe.serverPort();
    probe.close();

    m_stopping = false;
    m_output.clear();
    m_error.clear();
    emit stateChanged();
    m_process.start(binary, {"--port", QString::number(port), "--path", m_dataDirectory});
}

void Engine::stop() {
    if (m_process.state() == QProcess::NotRunning)
        return;
    m_stopping = true;
    if (m_port > 0) {
        // A blocking shutdown on quit: the engine flushes and closes its files.
        QNetworkAccessManager network;
        QNetworkRequest request(QUrl(QString("http://127.0.0.1:%1/shutdown").arg(m_port)));
        request.setHeader(QNetworkRequest::ContentTypeHeader, "application/json");
        request.setTransferTimeout(2000);
        QNetworkReply *reply = network.post(request, QByteArray("{}"));
        QEventLoop loop;
        connect(reply, &QNetworkReply::finished, &loop, &QEventLoop::quit);
        loop.exec();
        reply->deleteLater();
    }
    // Closing stdin is the engine's other cue to quit (ENGINE_EXIT_ON_STDIN_EOF).
    m_process.closeWriteChannel();
    if (!m_process.waitForFinished(4000)) {
        m_process.terminate();
        if (!m_process.waitForFinished(2000))
            m_process.kill();
    }
    m_port = 0;
}

void Engine::restart() {
    stop();
    start();
}

void Engine::get(const QString &path, Reply reply) {
    request("GET", path, {}, std::move(reply));
}

void Engine::post(const QString &path, const QJsonObject &body, Reply reply) {
    request("POST", path, QJsonDocument(body).toJson(QJsonDocument::Compact), std::move(reply));
}

void Engine::request(const QByteArray &method, const QString &path, const QByteArray &body, Reply reply) {
    if (m_port <= 0) {
        if (reply)
            reply(QJsonObject{{"error", tr("The torrent engine is not running.")}}, 0);
        return;
    }
    QNetworkRequest request(QUrl(QString("http://127.0.0.1:%1%2").arg(m_port).arg(path)));
    request.setHeader(QNetworkRequest::ContentTypeHeader, "application/json");
    QNetworkReply *networkReply = m_network.sendCustomRequest(request, method, body);
    connect(networkReply, &QNetworkReply::finished, this, [networkReply, reply] {
        networkReply->deleteLater();
        if (!reply)
            return;
        const int status = networkReply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
        const QJsonDocument document = QJsonDocument::fromJson(networkReply->readAll());
        QJsonObject object = document.object();
        if (status == 0 && !object.contains("error"))
            object.insert("error", networkReply->errorString());
        reply(object, status);
    });
}
