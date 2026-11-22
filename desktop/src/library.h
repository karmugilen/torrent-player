#pragma once

#include <QAbstractListModel>
#include <QHash>
#include <QJsonArray>
#include <QJsonObject>
#include <QSet>
#include <QTimer>
#include <QVariantMap>

class Engine;
class QVideoSink;

// The user's torrents: kept on disk across restarts, re-added to the engine
// whenever it starts, polled for progress, and shown as the home screen's
// cards. Finished files are linked (or copied) into ~/Downloads/Torrent Player,
// because the engine keeps its own files in its data directory.
class Library : public QAbstractListModel {
    Q_OBJECT
    Q_PROPERTY(int count READ count NOTIFY countChanged)
    Q_PROPERTY(int revision READ revision NOTIFY revisionChanged)
    Q_PROPERTY(double downloadSpeed READ downloadSpeed NOTIFY revisionChanged)
    Q_PROPERTY(double uploadSpeed READ uploadSpeed NOTIFY revisionChanged)
    Q_PROPERTY(int activeCount READ activeCount NOTIFY revisionChanged)
    Q_PROPERTY(QString saveDirectory READ saveDirectory CONSTANT)

public:
    enum Role {
        KeyRole = Qt::UserRole + 1,
        NameRole,
        ProgressRole,
        DownloadSpeedRole,
        UploadSpeedRole,
        PeersRole,
        SizeRole,
        DownloadedRole,
        StateRole,
        EtaRole,
        ThumbnailRole,
        ErrorRole,
        HasVideoRole,
        AddedRole,
    };

    struct Entry {
        // Saved.
        QString key; // info hash, lowercase hex
        QString magnet;
        QString name;
        QList<int> selected;
        QJsonArray files; // index, name, path, length, type
        bool paused = false;
        qint64 addedAt = 0;
        QSet<int> exported;
        QString thumbnail;
        QJsonObject positions; // file index -> last position in ms
        int lastFile = -1;

        // Live, from the engine.
        QString id;
        QJsonArray liveFiles;
        double progress = 0;
        double downloadSpeed = 0;
        double uploadSpeed = 0;
        int peers = 0;
        qint64 downloaded = 0;
        double etaMs = 0;
        bool ready = false;
        bool done = false;
        bool checking = false;
        bool configured = false;
        QString error;
        QSet<int> exporting;
        int thumbnailAttempts = 0;
        bool thumbnailBusy = false;
        qint64 thumbnailTriedAt = 0;
    };

    Library(Engine *engine, const QString &dataDirectory, const QString &saveDirectory, QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    int count() const { return m_entries.size(); }
    int revision() const { return m_revision; }
    double downloadSpeed() const;
    double uploadSpeed() const;
    int activeCount() const;
    QString saveDirectory() const { return m_saveDirectory; }

    bool contains(const QString &key) const;
    const Entry *entry(const QString &key) const;

    // A torrent the add dialog prepared in the engine, now kept and downloaded.
    QString adopt(const QString &id, const QString &key, const QString &magnet, const QString &name,
                  const QJsonArray &files, const QList<int> &selected, const QByteArray &torrentData);

    Q_INVOKABLE QVariantMap details(const QString &key) const;
    Q_INVOKABLE void play(const QString &key, int fileIndex);
    Q_INVOKABLE void pause(const QString &key);
    Q_INVOKABLE void resume(const QString &key);
    Q_INVOKABLE void togglePause(const QString &key);
    Q_INVOKABLE void retry(const QString &key);
    Q_INVOKABLE void remove(const QString &key, bool deleteFiles);
    Q_INVOKABLE void setFileSelected(const QString &key, int fileIndex, bool selected);
    Q_INVOKABLE void copyMagnet(const QString &key) const;
    Q_INVOKABLE void copyText(const QString &text) const;
    Q_INVOKABLE QString clipboardText() const;
    // Hands the stream to mpv, for formats the built-in player can't decode.
    Q_INVOKABLE bool openInMpv(const QString &url, qint64 startMs, const QString &title) const;
    Q_INVOKABLE void openFolder(const QString &key) const;
    Q_INVOKABLE void savePosition(const QString &key, int fileIndex, qint64 positionMs);
    Q_INVOKABLE void captureThumbnail(const QString &key, QObject *videoSink);
    // First video to play: the one last watched, else the largest selected video.
    // Where a fully downloaded file is on disk, or empty while it isn't.
    Q_INVOKABLE QString finishedFilePath(const QString &key, int fileIndex) const;
    Q_INVOKABLE int defaultVideo(const QString &key) const;

    // Saved state, exposed for tests.
    static QJsonObject toJson(const Entry &entry);
    static Entry fromJson(const QJsonObject &object);
    static QString displayState(const Entry &entry);

    QString partPath(const QString &key, int fileIndex) const;
    QString exportPath(const Entry &entry, int fileIndex) const;

signals:
    void countChanged();
    void revisionChanged();
    // Everything the player needs to open a file.
    void playReady(const QVariantMap &media);
    void message(const QString &text);

private:
    void load();
    void save();
    void restoreAll();
    void attach(Entry &entry);
    void configure(const QString &key);
    void poll();
    void update(Entry &entry, const QJsonObject &status);
    void exportFinished(Entry &entry);
    void makeThumbnail(Entry &entry);
    void finishThumbnail(const QString &key, const QString &path);
    int row(const QString &key) const;
    Entry *find(const QString &key);
    void touch(const QString &key);
    QString torrentFilePath(const QString &key) const;
    QString thumbnailPath(const QString &key) const;
    QVariantList subtitlesFor(const Entry &entry, int videoIndex) const;

    Engine *m_engine;
    QString m_dataDirectory;
    QString m_saveDirectory;
    QList<Entry> m_entries;
    QTimer m_poll;
    int m_revision = 0;
    int m_streamPort = 0;
    QString m_playingKey;
};
