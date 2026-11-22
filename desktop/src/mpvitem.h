#pragma once

#include <QHash>
#include <QQuickFramebufferObject>
#include <QVariantList>
#include <memory>

struct mpv_handle;
struct mpv_event;
struct MpvShared;

// libmpv drawn into the Qt Quick scene through mpv's OpenGL render API. Options
// follow the Android player (PlayerActivity.initMpv): small demuxer cache, since the
// engine already holds the data; no network timeout; keyframe seeks.
// Times are in milliseconds; volume is 0..1.
class MpvPlayer : public QQuickFramebufferObject {
    Q_OBJECT
    Q_PROPERTY(QString source READ source WRITE setSource NOTIFY sourceChanged)
    // Where the next source starts, in ms (resume). Set before source.
    Q_PROPERTY(qint64 startMs READ startMs WRITE setStartMs NOTIFY startMsChanged)
    Q_PROPERTY(qint64 position READ position NOTIFY positionChanged)
    Q_PROPERTY(qint64 duration READ duration NOTIFY durationChanged)
    Q_PROPERTY(bool paused READ paused WRITE setPaused NOTIFY pausedChanged)
    // Waiting for data: opening, paused for cache, or seeking.
    Q_PROPERTY(bool buffering READ buffering NOTIFY bufferingChanged)
    Q_PROPERTY(bool seeking READ seeking NOTIFY bufferingChanged)
    Q_PROPERTY(bool loaded READ loaded NOTIFY bufferingChanged)
    Q_PROPERTY(bool seekable READ seekable NOTIFY seekableChanged)
    Q_PROPERTY(bool eof READ eof NOTIFY eofChanged)
    // How far mpv has read ahead, in ms of the video (demuxer-cache-time).
    Q_PROPERTY(qint64 cacheEndMs READ cacheEndMs NOTIFY cacheEndMsChanged)
    Q_PROPERTY(double volume READ volume WRITE setVolume NOTIFY volumeChanged)
    Q_PROPERTY(bool muted READ muted WRITE setMuted NOTIFY mutedChanged)
    Q_PROPERTY(double speed READ speed WRITE setSpeed NOTIFY speedChanged)
    // Each track: {id, title, lang, codec, external, externalFilename, label}.
    Q_PROPERTY(QVariantList audioTracks READ audioTracks NOTIFY tracksChanged)
    Q_PROPERTY(QVariantList subtitleTracks READ subtitleTracks NOTIFY tracksChanged)
    // mpv track ids; -1 is off.
    Q_PROPERTY(int audioId READ audioId WRITE setAudioId NOTIFY audioIdChanged)
    Q_PROPERTY(int subtitleId READ subtitleId WRITE setSubtitleId NOTIFY subtitleIdChanged)
    Q_PROPERTY(QString errorString READ errorString NOTIFY errorStringChanged)

public:
    explicit MpvPlayer(QQuickItem *parent = nullptr);
    ~MpvPlayer() override;

    Renderer *createRenderer() const override;

    QString source() const { return m_source; }
    void setSource(const QString &source);
    qint64 startMs() const { return m_startMs; }
    void setStartMs(qint64 ms);
    qint64 position() const { return m_position; }
    qint64 duration() const { return m_duration; }
    bool paused() const { return m_paused; }
    void setPaused(bool paused);
    bool buffering() const;
    bool seeking() const { return m_seeking; }
    bool loaded() const { return m_loaded; }
    bool seekable() const { return m_seekable; }
    bool eof() const { return m_eof; }
    qint64 cacheEndMs() const { return m_cacheEnd; }
    double volume() const { return m_volume; }
    void setVolume(double volume);
    bool muted() const { return m_muted; }
    void setMuted(bool muted);
    double speed() const { return m_speed; }
    void setSpeed(double speed);
    QVariantList audioTracks() const { return m_audioTracks; }
    QVariantList subtitleTracks() const { return m_subtitleTracks; }
    int audioId() const { return m_audioId; }
    void setAudioId(int id);
    int subtitleId() const { return m_subtitleId; }
    void setSubtitleId(int id);
    QString errorString() const { return m_error; }

    Q_INVOKABLE void play();
    Q_INVOKABLE void pause();
    Q_INVOKABLE void stop();
    // Keyframe seek unless exact.
    Q_INVOKABLE void seek(qint64 ms, bool exact = false);
    Q_INVOKABLE void frameStep();
    // Adds and selects an external subtitle (file path or URL).
    Q_INVOKABLE void addSubtitle(const QString &pathOrUrl, const QString &title = QString());
    // Saves the current video frame (no subtitles); emits screenshotSaved.
    Q_INVOKABLE void screenshotTo(const QString &path);

    // Track label as the menus show it: "title · lang", else "Track n".
    static QString trackLabel(const QString &title, const QString &lang, int number);

signals:
    void sourceChanged();
    void startMsChanged();
    void positionChanged();
    void durationChanged();
    void pausedChanged();
    void bufferingChanged();
    void seekableChanged();
    void eofChanged();
    void cacheEndMsChanged();
    void volumeChanged();
    void mutedChanged();
    void speedChanged();
    void tracksChanged();
    void audioIdChanged();
    void subtitleIdChanged();
    void errorStringChanged();
    void fileLoaded();
    void screenshotSaved(const QString &path, bool ok);

private slots:
    void onMpvEvents();
    void onRenderReady();
    void loadSource();

private:
    friend class MpvRenderer;
    void handleEvent(mpv_event *event);
    void handleProperty(const char *name, int format, void *data);
    void setProperty(const char *name, const QString &value);
    void command(const QStringList &args, quint64 userdata = 0);
    void readTracks(void *node);

    std::shared_ptr<mpv_handle> m_mpv;
    std::shared_ptr<MpvShared> m_shared;
    QString m_source;
    qint64 m_startMs = 0;
    qint64 m_position = 0;
    qint64 m_duration = 0;
    bool m_paused = false;
    bool m_pausedForCache = false;
    bool m_seeking = false;
    bool m_loaded = false;
    bool m_seekable = false;
    bool m_eof = false;
    qint64 m_cacheEnd = 0;
    double m_volume = 1.0;
    bool m_muted = false;
    double m_speed = 1.0;
    QVariantList m_audioTracks;
    QVariantList m_subtitleTracks;
    int m_audioId = -1;
    int m_subtitleId = -1;
    QString m_error;
    bool m_renderReady = false;
    bool m_loadPending = false;
    bool m_renderWaitOver = false;
    QHash<quint64, QString> m_screenshots;
    quint64 m_nextReply = 1;
};
