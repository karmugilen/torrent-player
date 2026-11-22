#include "engine.h"

#include <QCoreApplication>
#include <QDir>
#include <QEventLoop>
#include <QFileInfo>
#include <QJsonDocument>
#include <QNetworkReply>
#include <QRegularExpression>
#include <QTcpServer>
#include <QTimer>

Engine::Engine(const QString &dataDirectory, QObject *parent)
    : QObject(parent), m_dataDirectory(dataDirectory) {
    m_process.setProcessChannelMode(QProcess::MergedChannels);
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
    const QString fromEnvironment = qEnvironmentVariable("TORRENTPLAYER_ENGINE");
    const QStringList candidates{fromEnvironment,
                                 QCoreApplication::applicationDirPath() + "/engine-host",
                                 "/usr/lib/torrentplayer/engine-host"};
    for (const QString &candidate : candidates)
        if (!candidate.isEmpty() && QFileInfo(candidate).isExecutable())
            return candidate;
    return {};
}

void Engine::fail(const QString &message) {
    m_port = 0;
    m_error = message;
    emit stateChanged();
}

void Engine::start() {
    if (m_process.state() != QProcess::NotRunning)
        return;
    const QString binary = findBinary();
    if (binary.isEmpty()) {
        fail(tr("The torrent engine (engine-host) is missing. Reinstall Torrent Player."));
        return;
    }
    QDir().mkpath(m_dataDirectory);

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
