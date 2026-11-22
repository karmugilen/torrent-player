#pragma once

#include <QJsonArray>
#include <QJsonObject>
#include <QNetworkAccessManager>
#include <QObject>
#include <QTimer>
#include <QVariantList>

class Engine;
class Library;

// One torrent being added: a magnet, a link to a .torrent, or a .torrent file
// goes to the engine, which finds the file list; the user picks files and
// either downloads them or starts playing at once.
class AddSession : public QObject {
    Q_OBJECT
    Q_PROPERTY(QString state READ state NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QString name READ name NOTIFY changed)
    Q_PROPERTY(QString existingKey READ existingKey NOTIFY changed)
    Q_PROPERTY(QVariantList files READ files NOTIFY changed)
    Q_PROPERTY(int selectedCount READ selectedCount NOTIFY changed)
    Q_PROPERTY(double selectedBytes READ selectedBytes NOTIFY changed)
    Q_PROPERTY(double freeBytes READ freeBytes NOTIFY changed)
    Q_PROPERTY(bool selectedHasVideo READ selectedHasVideo NOTIFY changed)
    Q_PROPERTY(int peers READ peers NOTIFY changed)
    Q_PROPERTY(int waitedSeconds READ waitedSeconds NOTIFY changed)

public:
    AddSession(Engine *engine, Library *library, QObject *parent = nullptr);

    // idle, starting (waiting for the engine), loading (fetching the .torrent), fetching (asking peers for the
    // file list), choose, error.
    QString state() const { return m_state; }
    QString error() const { return m_error; }
    QString name() const { return m_name; }
    QString existingKey() const { return m_existingKey; }
    QVariantList files() const;
    int selectedCount() const { return m_selected.size(); }
    double selectedBytes() const;
    double freeBytes() const;
    bool selectedHasVideo() const;
    int peers() const { return m_peers; }
    int waitedSeconds() const { return m_waited; }

    // Which files start ticked: videos and their subtitles, or everything
    // when there is no video. Exposed for tests.
    static QList<int> defaultSelection(const QJsonArray &files);

    Q_INVOKABLE void start(const QString &input);
    Q_INVOKABLE void toggle(int index);
    Q_INVOKABLE void selectAll();
    Q_INVOKABLE void selectNone();
    Q_INVOKABLE void selectVideos();
    // Keeps the torrent in the library; playNow also opens the player.
    Q_INVOKABLE void confirm(bool playNow);
    Q_INVOKABLE void cancel();

signals:
    void changed();
    void added(const QString &key, bool playNow);

private:
    void addToEngine(const QJsonObject &body);
    void pollFiles();
    void fail(const QString &message);
    void reset();

    Engine *m_engine;
    Library *m_library;
    QNetworkAccessManager m_network;
    QTimer m_poll;
    QString m_state = "idle";
    QString m_error;
    QString m_name;
    QString m_id;
    QString m_key;
    QString m_magnet;
    QString m_existingKey;
    QByteArray m_torrentData;
    QJsonObject m_pendingBody;
    QJsonArray m_files;
    QList<int> m_selected;
    int m_peers = 0;
    int m_waited = 0;
    int m_generation = 0;
    qint64 m_startedAt = 0;
};
