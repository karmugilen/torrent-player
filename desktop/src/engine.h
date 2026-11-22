#pragma once

#include <QJsonObject>
#include <QNetworkAccessManager>
#include <QObject>
#include <QProcess>
#include <functional>

// Runs the Rust torrent engine (engine-host, from the Android app's engine-rs)
// as a child process and talks to its localhost HTTP control API.
class Engine : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool ready READ ready NOTIFY stateChanged)
    Q_PROPERTY(QString error READ error NOTIFY stateChanged)
    Q_PROPERTY(QString dataDirectory READ dataDirectory CONSTANT)

public:
    // reply: JSON body (or {"error": ...}), HTTP status (0 when unreachable).
    using Reply = std::function<void(const QJsonObject &body, int status)>;

    explicit Engine(const QString &dataDirectory, QObject *parent = nullptr);
    ~Engine() override;

    bool ready() const { return m_port > 0; }
    QString error() const { return m_error; }
    QString dataDirectory() const { return m_dataDirectory; }
    int port() const { return m_port; }

    static QString findBinary();
    void start();
    // Asks the engine to stop and waits briefly, so part files are closed cleanly.
    void stop();
    Q_INVOKABLE void restart();

    void get(const QString &path, Reply reply = {});
    void post(const QString &path, const QJsonObject &body, Reply reply = {});

signals:
    void stateChanged();
    // Emitted every time the engine (re)starts, so the library can re-add its torrents.
    void started();

private:
    void request(const QByteArray &method, const QString &path, const QByteArray &body, Reply reply);
    void fail(const QString &message);

    QString m_dataDirectory;
    QProcess m_process;
    QNetworkAccessManager m_network;
    QByteArray m_output;
    QString m_error;
    int m_port = 0;
    bool m_stopping = false;
};
