#include "subtitles.h"

#include <QFile>
#include <QNetworkReply>
#include <QRegularExpression>
#include <QStringDecoder>
#include <QUrl>
#include <algorithm>

Subtitles::Subtitles(QObject *parent) : QObject(parent) {}

static qint64 timestamp(const QString &text) {
    // 01:02:03,456 or 01:02:03.456 or (WebVTT) 02:03.456
    static const QRegularExpression re(R"(^(?:(\d+):)?(\d{1,2}):(\d{2})[,.](\d{1,3})$)");
    const auto m = re.match(text.trimmed());
    if (!m.hasMatch())
        return -1;
    const qint64 h = m.captured(1).toLongLong(), mi = m.captured(2).toLongLong(), s = m.captured(3).toLongLong();
    const qint64 ms = m.captured(4).leftJustified(3, '0').toLongLong();
    return ((h * 60 + mi) * 60 + s) * 1000 + ms;
}

static QString decode(const QByteArray &data) {
    // Most subtitle files are UTF-8; older ones are often Windows-1252, which
    // shows as replacement characters when read as UTF-8.
    QStringDecoder utf8(QStringDecoder::Utf8);
    QString text = utf8(data);
    if (utf8.hasError()) {
        QStringDecoder latin(QStringDecoder::Latin1);
        text = latin(data);
    }
    if (text.startsWith(QChar(0xfeff)))
        text.remove(0, 1);
    return text;
}

QVector<Subtitles::Cue> Subtitles::parse(const QByteArray &data) {
    QVector<Cue> cues;
    QString text = decode(data);
    text.replace("\r\n", "\n").replace('\r', '\n');
    static const QRegularExpression tags("<[^>]*>|\\{[^}]*\\}");
    const QStringList blocks = text.split(QRegularExpression("\n\\s*\n"), Qt::SkipEmptyParts);
    for (const QString &block : blocks) {
        QStringList lines = block.split('\n');
        int timing = -1;
        for (int i = 0; i < lines.size() && i < 3; ++i)
            if (lines[i].contains("-->")) {
                timing = i;
                break;
            }
        if (timing < 0)
            continue;
        const QStringList parts = lines[timing].split("-->");
        if (parts.size() != 2)
            continue;
        // WebVTT puts cue settings after the end time.
        const qint64 start = timestamp(parts[0]);
        const qint64 end = timestamp(parts[1].trimmed().section(' ', 0, 0));
        if (start < 0 || end < start)
            continue;
        QStringList body = lines.mid(timing + 1);
        QString cueText = body.join('\n');
        cueText.remove(tags);
        cueText.replace("\\N", "\n");
        cueText = cueText.trimmed();
        if (!cueText.isEmpty())
            cues.append({start, end, cueText});
    }
    std::sort(cues.begin(), cues.end(), [](const Cue &a, const Cue &b) { return a.start < b.start; });
    return cues;
}

void Subtitles::setCues(const QVector<Cue> &cues, const QString &label) {
    m_cues = cues;
    m_label = cues.isEmpty() ? QString() : label;
    m_error = cues.isEmpty() ? tr("No subtitles found in %1").arg(label) : QString();
    emit loadedChanged();
}

void Subtitles::clear() {
    ++m_generation;
    m_cues.clear();
    m_label.clear();
    m_error.clear();
    emit loadedChanged();
}

void Subtitles::load(const QString &source, const QString &label) {
    const int generation = ++m_generation;
    if (source.startsWith("http://") || source.startsWith("https://")) {
        QNetworkReply *reply = m_network.get(QNetworkRequest(QUrl(source)));
        connect(reply, &QNetworkReply::finished, this, [this, reply, label, generation] {
            reply->deleteLater();
            if (generation != m_generation)
                return;
            if (reply->error() != QNetworkReply::NoError) {
                m_cues.clear();
                m_error = tr("Could not load subtitles: %1").arg(reply->errorString());
                emit loadedChanged();
                return;
            }
            setCues(parse(reply->readAll()), label);
        });
        return;
    }
    QFile file(source.startsWith("file:") ? QUrl(source).toLocalFile() : source);
    if (!file.open(QIODevice::ReadOnly)) {
        m_cues.clear();
        m_error = tr("Could not open %1").arg(file.fileName());
        emit loadedChanged();
        return;
    }
    setCues(parse(file.readAll()), label);
}

QString Subtitles::textAt(qint64 positionMs) const {
    // Cues can overlap, so show every cue that covers the position.
    auto it = std::upper_bound(m_cues.begin(), m_cues.end(), positionMs,
                               [](qint64 p, const Cue &c) { return p < c.start; });
    QStringList shown;
    for (auto back = it; back != m_cues.begin();) {
        --back;
        if (back->end >= positionMs)
            shown.prepend(back->text);
        if (positionMs - back->start > 60000)
            break;
    }
    return shown.join('\n');
}
