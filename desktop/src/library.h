#pragma once

#include <QAbstractListModel>
#include <QHash>
#include <QJsonArray>
#include <QJsonObject>
#include <QSet>
#include <QTimer>
#include <QVariantMap>
#include <functional>

class Engine;
class QVideoSink;
class Settings;
class Policy;

// The user's torrents: kept on disk across restarts, re-added to the engine
// whenever it starts, polled for progress, and shown as the home screen's
// cards. Finished files are linked (or copied) into ~/Downloads/Torrent Player,
// because the engine keeps its own files in its data directory. Direct Play
// torrents only stream (RAM cache): nothing goes to Downloads, they start
// dropped after a restart and are dropped again when idle, as on Android.
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
        DirectPlayRole,
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
        QJsonObject durations; // file index -> length of the video in ms, when the player knew it
        int lastFile = -1;
        bool directPlay = false; // stream only (storageMode "stream"); nothing saved to Downloads

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
        bool attaching = false;
        bool seeding = false;
        bool idleDropReady = false;
        int playbackReaders = 0;
        QString error;
        QSet<int> exporting;
        int thumbnailAttempts = 0;
        bool thumbnailBusy = false;
        qint64 thumbnailTriedAt = 0;
    };

    Library(Engine *engine, const QString &dataDirectory, const QString &saveDirectory, QObject *parent = nullptr);
    // RAM cache size, idle drop and the active-transfer count go through these settings.
    // Without them the policy defaults apply.
    void setSettings(Settings *settings);

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
    // directPlay: stream only (Android startDirectPlay), else a download.
    QString adopt(const QString &id, const QString &key, const QString &magnet, const QString &name,
                  const QJsonArray &files, const QList<int> &selected, const QByteArray &torrentData,
                  bool directPlay = false);

    Q_INVOKABLE QVariantMap details(const QString &key) const;
    Q_INVOKABLE void play(const QString &key, int fileIndex);
    Q_INVOKABLE void pause(const QString &key);
    Q_INVOKABLE void resume(const QString &key);
    Q_INVOKABLE void togglePause(const QString &key);
    // Ends a Direct Play session: the engine forgets it, peers disconnect, RAM is freed.
    // Play (or Resume) starts it again.
    Q_INVOKABLE void drop(const QString &key);
    // A Direct Play torrent becomes a download: saved to disk and exported to Downloads.
    Q_INVOKABLE void downloadInstead(const QString &key);
    Q_INVOKABLE void retry(const QString &key);
    Q_INVOKABLE void remove(const QString &key, bool deleteFiles);
    Q_INVOKABLE void setFileSelected(const QString &key, int fileIndex, bool selected);
    Q_INVOKABLE void copyMagnet(const QString &key) const;
    Q_INVOKABLE void copyText(const QString &text) const;
    Q_INVOKABLE QString clipboardText() const;
    // Hands the stream to mpv, for formats the built-in player can't decode.
    Q_INVOKABLE bool openInMpv(const QString &url, qint64 startMs, const QString &title) const;
    Q_INVOKABLE void openFolder(const QString &key) const;
    // The player closed: keeps the place (0 within 3 s of the end, Android withPlaybackPosition),
    // unfocuses the file and clears file-only playback. durationMs < 0 keeps the known length.
    Q_INVOKABLE void savePosition(const QString &key, int fileIndex, qint64 positionMs, qint64 durationMs = -1);
    // The player learned the video's length (used for the resume byte hint).
    Q_INVOKABLE void setPlaybackDuration(const QString &key, int fileIndex, qint64 durationMs);
    Q_INVOKABLE void captureThumbnail(const QString &key, QObject *videoSink);
    // Where the home card's picture goes; a frame saved there is picked up on the next poll.
    Q_INVOKABLE QString thumbnailPath(const QString &key) const;
    // First video to play: the one last watched, else the largest selected video.
    // Where a fully downloaded file is on disk, or empty while it isn't.
    Q_INVOKABLE QString finishedFilePath(const QString &key, int fileIndex) const;
    Q_INVOKABLE int defaultVideo(const QString &key) const;

    // Saved state, exposed for tests.
    static QJsonObject toJson(const Entry &entry);
    static Entry fromJson(const QJsonObject &object);
    static QString displayState(const Entry &entry);

    // Android's play rules, pure, exposed for tests.
    // The saved place: 0 within 3 s of the end, else clamped to [0, duration].
    static qint64 savedPosition(qint64 positionMs, qint64 durationMs);
    // Approximate byte to start fetching at (PlaybackLaunch.kt resumeByteHint); -1 when unknown.
    static qint64 resumeByteHint(qint64 length, qint64 positionMs, qint64 durationMs);
    // The video Play next opens after endedIndex in natural order, or -1 (nextVideoAfter).
    static int nextVideoAfter(const Entry &entry, int endedIndex);
    // The episode to warm while endedIndex plays, or -1 (nextEpisodeToWarm).
    static int nextEpisodeToWarm(const Entry &entry, int playingIndex, bool playNext);
    // Should the engine's idle signal drop this Direct Play torrent (shouldDropIdleDirectPlay).
    static bool shouldDropIdle(const Entry &entry, int idleMinutes, bool playing);
    // Torrents running now: not paused, without error, unfinished or seeding (at least 1).
    static int activeTransfers(const QList<Entry> &entries);
    // The /play body for this file, with the policy's play flags.
    static QJsonObject playBody(const Entry &entry, int fileIndex, const QJsonObject &playPolicy);

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
    void startPlay(const QString &key, int fileIndex);
    void awaitPreload(const QString &key, const QString &id, int fileIndex, qint64 length, qint64 deadline,
                      std::function<void()> done);
    void endPlayback();
    void dropEntry(Entry &entry, bool idle);
    QJsonObject playPolicy() const;
    int idleDropMinutes() const;
    qint64 streamCacheBytes() const;
    void poll();
    void update(Entry &entry, const QJsonObject &status);
    void exportFinished(Entry &entry);
    void makeThumbnail(Entry &entry);
    void finishThumbnail(const QString &key, const QString &path);
    int row(const QString &key) const;
    Entry *find(const QString &key);
    void touch(const QString &key);
    QString torrentFilePath(const QString &key) const;
    QVariantList subtitlesFor(const Entry &entry, int videoIndex) const;

    Engine *m_engine;
    QString m_dataDirectory;
    QString m_saveDirectory;
    QList<Entry> m_entries;
    QTimer m_poll;
    int m_revision = 0;
    int m_streamPort = 0;
    QString m_playingKey;
    int m_playingFile = -1;
    QString m_playingId;
    QString m_fileOnlyId; // engine id with file-only playback on
    QHash<QString, int> m_pendingPlay; // key -> file to play once the torrent is configured
    QSet<QString> m_idleDropped;       // engine ids already dropped for idling
    Settings *m_settings = nullptr;
    int m_activeTransfers = -1;
};
