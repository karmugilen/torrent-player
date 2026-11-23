#pragma once

#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonObject>
#include <QList>
#include <QObject>
#include <QPointer>
#include <QTimer>
#include <QVariant>

class Engine;

// The player's side of the engine, as the Android player does it (PlayerActivity):
// tells the engine when the length is known, prefetches the bytes around a seek
// target, and polls which parts of the playing file are downloaded so the seek
// bar can draw them and a drag can decide whether to seek live.
class PlaybackControl : public QObject {
    Q_OBJECT
    // The engine context property.
    Q_PROPERTY(QObject *engine READ engineObject WRITE setEngineObject NOTIFY engineChanged)
    // The player's media map from library.play: engineId, fileIndex, length.
    Q_PROPERTY(QVariantMap media READ media WRITE setMedia NOTIFY mediaChanged)
    Q_PROPERTY(qint64 durationMs READ durationMs WRITE setDurationMs NOTIFY durationMsChanged)
    // Downloaded parts of this file: [{from, to}] as fractions 0..1, merged.
    Q_PROPERTY(QVariantList readyRanges READ readyRanges NOTIFY readyRangesChanged)
    // The whole file is downloaded.
    Q_PROPERTY(bool complete READ complete NOTIFY readyRangesChanged)
    // Swarm line for the buffering label.
    Q_PROPERTY(int peers READ peers NOTIFY statusChanged)
    Q_PROPERTY(double downloadSpeed READ downloadSpeed NOTIFY statusChanged)

public:
    struct Span {
        double from = 0;
        double to = 0;
        double done = 0; // verified share of the pieces under this span
    };

    explicit PlaybackControl(QObject *parent = nullptr);

    QObject *engineObject() const;
    void setEngineObject(QObject *engine);
    QVariantMap media() const { return m_media; }
    void setMedia(const QVariantMap &media);
    qint64 durationMs() const { return m_duration; }
    void setDurationMs(qint64 ms);
    QVariantList readyRanges() const;
    bool complete() const { return m_complete; }
    int peers() const { return m_peers; }
    double downloadSpeed() const { return m_downloadSpeed; }

    // true when the spot at this fraction of the file is downloaded, false when it
    // isn't, undefined while nothing is known yet.
    Q_INVOKABLE QVariant readyAt(double fraction) const;
    // Asks the engine for the bytes around targetMs, unless they are downloaded.
    // Waits 150 ms so only the spot a drag rests on is fetched.
    Q_INVOKABLE void prefetchFor(qint64 targetMs);
    // The same, at once (arrow-key skips).
    Q_INVOKABLE void prefetchNow(qint64 targetMs);
    // One step of a seek-bar drag, as Android's PlayerGestures.scrub:
    // {label: "1:23  [+0:10]", note: "will buffer" or "", action: "seek"|"wait"|"prefetch"}.
    // Records the time of each "seek" so live seeks go out at most every 250 ms.
    Q_INVOKABLE QVariantMap scrub(qint64 targetMs, qint64 originMs, qint64 positionMs, qint64 cacheEndMs, bool mpvSeeking);
    Q_INVOKABLE void refresh();

    // Pure parts, tested.
    static constexpr qint64 PrefetchBefore = 2LL << 20;
    static constexpr qint64 PrefetchLength = 10LL << 20;
    static constexpr qint64 LiveSeekEveryMs = 250;
    // Byte offset to prefetch from for a time: the bitrate estimate, 2 MiB earlier.
    static qint64 prefetchOffset(qint64 fileLength, qint64 targetMs, qint64 durationMs);
    // Where a file starts in the torrent: the lengths of the files before it.
    static qint64 fileOffset(const QJsonArray &files, int fileIndex);
    // The /pieces buckets that cover the file, as fractions of it.
    static QList<Span> spansFromPieces(const QJsonObject &pieces, qint64 fileOffset, qint64 fileLength);
    static QVariant readyAt(const QList<Span> &spans, double fraction);
    // Fully downloaded spans, adjacent ones joined.
    static QList<QPair<double, double>> mergeReady(const QList<Span> &spans);
    static QString clock(qint64 ms);
    static QVariantMap scrubDecision(qint64 targetMs, qint64 originMs, qint64 positionMs, qint64 cacheEndMs,
                                     QVariant ready, qint64 sinceLastSeekMs, bool mpvSeeking);
    // Seek-bar playhead after release: drag target while dragging, else pending seek
    // until mpv catches up (Android pendingSeek), else the real position. pendingMs < 0 = none.
    Q_INVOKABLE static qint64 shownPlayheadMs(bool dragging, qint64 dragTargetMs, qint64 pendingMs, qint64 positionMs);
    // True when pending seek can clear: within catchupMs of the target and not seeking.
    Q_INVOKABLE static bool pendingSeekCaughtUp(qint64 pendingMs, qint64 positionMs, bool seeking, qint64 catchupMs = 3000);
    static constexpr qint64 PendingSeekTimeoutMs = 4000;

signals:
    void engineChanged();
    void mediaChanged();
    void durationMsChanged();
    void readyRangesChanged();
    void statusChanged();

private:
    void sendLengthKnown();
    void setSpans(const QList<Span> &spans);

    QPointer<Engine> m_engine;
    QVariantMap m_media;
    QString m_id;
    int m_fileIndex = -1;
    qint64 m_length = 0;
    qint64 m_duration = 0;
    bool m_lengthSent = false;
    QList<Span> m_spans;
    bool m_complete = false;
    int m_peers = 0;
    double m_downloadSpeed = 0;
    bool m_busy = false;
    QTimer m_poll;
    QTimer m_prefetch;
    qint64 m_prefetchTarget = 0;
    QElapsedTimer m_sinceSeek;
};
