#include "playback.h"

#include "engine.h"

#include <QtMath>

PlaybackControl::PlaybackControl(QObject *parent) : QObject(parent) {
    m_poll.setInterval(1000);
    connect(&m_poll, &QTimer::timeout, this, &PlaybackControl::refresh);
    m_prefetch.setSingleShot(true);
    m_prefetch.setInterval(150);
    connect(&m_prefetch, &QTimer::timeout, this, [this] { prefetchNow(m_prefetchTarget); });
}

QObject *PlaybackControl::engineObject() const { return m_engine; }

void PlaybackControl::setEngineObject(QObject *engine) {
    auto *typed = qobject_cast<Engine *>(engine);
    if (typed == m_engine)
        return;
    m_engine = typed;
    emit engineChanged();
    refresh();
    sendLengthKnown();
}

void PlaybackControl::setMedia(const QVariantMap &media) {
    m_media = media;
    m_id = media.value("engineId").toString();
    m_fileIndex = media.value("fileIndex", -1).toInt();
    m_length = qint64(media.value("length").toDouble());
    m_lengthSent = false;
    m_complete = false;
    m_spans.clear();
    emit mediaChanged();
    emit readyRangesChanged();
    if (m_id.isEmpty())
        m_poll.stop();
    else
        m_poll.start();
    refresh();
    sendLengthKnown();
}

void PlaybackControl::setDurationMs(qint64 ms) {
    if (ms == m_duration)
        return;
    m_duration = ms;
    emit durationMsChanged();
    sendLengthKnown();
}

void PlaybackControl::sendLengthKnown() {
    // Once per file: the engine stops fetching the container index first.
    if (m_lengthSent || m_duration <= 0 || !m_engine || m_id.isEmpty() || m_fileIndex < 0)
        return;
    m_lengthSent = true;
    m_engine->post("/length-known", {{"id", m_id}, {"fileIndex", m_fileIndex}});
}

QVariantList PlaybackControl::readyRanges() const {
    QVariantList list;
    for (const auto &range : mergeReady(m_spans))
        list.append(QVariantMap{{"from", range.first}, {"to", range.second}});
    return list;
}

QVariant PlaybackControl::readyAt(double fraction) const { return readyAt(m_spans, fraction); }

void PlaybackControl::prefetchFor(qint64 targetMs) {
    m_prefetchTarget = targetMs;
    m_prefetch.start();
}

void PlaybackControl::prefetchNow(qint64 targetMs) {
    m_prefetch.stop();
    if (!m_engine || m_id.isEmpty() || m_fileIndex < 0 || m_length <= 0 || m_duration <= 0)
        return;
    if (readyAt(double(targetMs) / double(m_duration)) == QVariant(true))
        return;
    m_engine->post("/prefetch", {{"id", m_id},
                                 {"fileIndex", m_fileIndex},
                                 {"offset", double(prefetchOffset(m_length, targetMs, m_duration))},
                                 {"length", double(PrefetchLength)}});
}

QVariantMap PlaybackControl::scrub(qint64 targetMs, qint64 originMs, qint64 positionMs, qint64 cacheEndMs, bool mpvSeeking) {
    const QVariant ready = m_duration > 0 ? readyAt(double(targetMs) / double(m_duration)) : QVariant();
    const qint64 since = m_sinceSeek.isValid() ? m_sinceSeek.elapsed() : LiveSeekEveryMs + 1;
    QVariantMap decision = scrubDecision(targetMs, originMs, positionMs, cacheEndMs, ready, since, mpvSeeking);
    const QString action = decision.value("action").toString();
    if (action == "seek")
        m_sinceSeek.start();
    else if (action == "prefetch")
        prefetchFor(targetMs);
    return decision;
}

void PlaybackControl::refresh() {
    if (!m_engine || m_id.isEmpty() || m_fileIndex < 0 || m_busy)
        return;
    m_busy = true;
    const QString id = m_id;
    const int index = m_fileIndex;
    QPointer<PlaybackControl> self(this);
    m_engine->get("/torrent/" + id, [self, id, index](const QJsonObject &status, int code) {
        if (!self)
            return;
        if (code != 200 || id != self->m_id || index != self->m_fileIndex || !self->m_engine) {
            self->m_busy = false;
            return;
        }
        self->m_peers = status.value("numPeers").toInt();
        self->m_downloadSpeed = status.value("downloadSpeed").toDouble();
        emit self->statusChanged();
        const QJsonArray files = status.value("files").toArray();
        qint64 length = 0;
        double progress = 0;
        for (const auto &value : files) {
            const QJsonObject f = value.toObject();
            if (f.value("index").toInt() != index)
                continue;
            length = qint64(f.value("length").toDouble());
            const qint64 verified = qint64(f.value("verifiedBytes").toDouble());
            progress = length > 0 && f.contains("verifiedBytes") ? double(verified) / double(length)
                                                                 : f.value("progress").toDouble();
        }
        if (length <= 0) {
            self->m_busy = false;
            return;
        }
        self->m_length = length;
        if (progress >= 1.0) {
            self->m_busy = false;
            self->m_complete = true;
            self->setSpans({Span{0, 1, 1}});
            return;
        }
        const qint64 offset = fileOffset(files, index);
        self->m_engine->get(QString("/pieces/%1?maxBuckets=1024").arg(id), [self, id, index, offset, length](const QJsonObject &pieces, int code) {
            if (!self)
                return;
            self->m_busy = false;
            if (code != 200 || id != self->m_id || index != self->m_fileIndex)
                return;
            self->setSpans(spansFromPieces(pieces, offset, length));
        });
    });
}

void PlaybackControl::setSpans(const QList<Span> &spans) {
    m_spans = spans;
    m_complete = !spans.isEmpty() && mergeReady(spans) == QList<QPair<double, double>>{{0.0, 1.0}};
    emit readyRangesChanged();
}

// ---------------------------------------------------------------- pure

qint64 PlaybackControl::prefetchOffset(qint64 fileLength, qint64 targetMs, qint64 durationMs) {
    if (fileLength <= 0 || durationMs <= 0)
        return 0;
    const double fraction = qBound(0.0, double(targetMs) / double(durationMs), 1.0);
    const qint64 estimate = qint64(double(fileLength) * fraction);
    return qBound<qint64>(0, estimate - PrefetchBefore, fileLength);
}

qint64 PlaybackControl::fileOffset(const QJsonArray &files, int fileIndex) {
    qint64 offset = 0;
    for (const auto &value : files) {
        const QJsonObject f = value.toObject();
        if (f.value("index").toInt() < fileIndex)
            offset += qint64(f.value("length").toDouble());
    }
    return offset;
}

QList<PlaybackControl::Span> PlaybackControl::spansFromPieces(const QJsonObject &pieces, qint64 fileOffset, qint64 fileLength) {
    QList<Span> spans;
    const qint64 pieceLength = qint64(pieces.value("pieceLength").toDouble());
    if (pieceLength <= 0 || fileLength <= 0)
        return spans;
    const qint64 end = fileOffset + fileLength;
    for (const auto &value : pieces.value("buckets").toArray()) {
        const QJsonObject b = value.toObject();
        const qint64 first = b.value("start").toInteger();
        const qint64 last = b.value("end").toInteger(first);
        const qint64 total = b.value("total").toInteger(last - first + 1);
        const qint64 a = qMax(first * pieceLength, fileOffset);
        const qint64 z = qMin((last + 1) * pieceLength, end);
        if (z <= a || total <= 0)
            continue;
        const double done = qBound(0.0, double(b.value("verified").toInteger()) / double(total), 1.0);
        spans.append({double(a - fileOffset) / double(fileLength), double(z - fileOffset) / double(fileLength), done});
    }
    return spans;
}

QVariant PlaybackControl::readyAt(const QList<Span> &spans, double fraction) {
    if (spans.isEmpty())
        return {};
    for (const Span &span : spans)
        if (fraction >= span.from && fraction <= span.to)
            return span.done >= 0.99;
    return false;
}

QList<QPair<double, double>> PlaybackControl::mergeReady(const QList<Span> &spans) {
    QList<QPair<double, double>> ranges;
    for (const Span &span : spans) {
        if (span.done < 0.99)
            continue;
        if (!ranges.isEmpty() && span.from <= ranges.last().second + 1e-9)
            ranges.last().second = qMax(ranges.last().second, span.to);
        else
            ranges.append({span.from, span.to});
    }
    return ranges;
}

QString PlaybackControl::clock(qint64 ms) {
    const qint64 s = qMax<qint64>(0, ms / 1000);
    if (s >= 3600)
        return QString("%1:%2:%3").arg(s / 3600).arg(s / 60 % 60, 2, 10, QChar('0')).arg(s % 60, 2, 10, QChar('0'));
    return QString("%1:%2").arg(s / 60).arg(s % 60, 2, 10, QChar('0'));
}

QVariantMap PlaybackControl::scrubDecision(qint64 targetMs, qint64 originMs, qint64 positionMs, qint64 cacheEndMs,
                                           QVariant ready, qint64 sinceLastSeekMs, bool mpvSeeking) {
    const qint64 delta = targetMs - originMs;
    const QString label = clock(targetMs) + "  [" + (delta >= 0 ? "+" : "-") + clock(qAbs(delta)) + "]";
    const bool inBuffer = targetMs >= positionMs - 5000 && targetMs <= cacheEndMs;
    const bool isReady = ready.isValid() && ready.toBool();
    const bool notReady = ready.isValid() && !ready.toBool();
    QString action;
    if (!isReady && !inBuffer)
        action = "prefetch";
    else if (sinceLastSeekMs > LiveSeekEveryMs && !mpvSeeking)
        action = "seek";
    else
        action = "wait";
    return {{"label", label}, {"note", notReady && !inBuffer ? tr("will buffer") : QString()}, {"action", action}};
}
