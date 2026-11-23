#include "series.h"

#include "links.h"

#include <QObject>
#include <QRegularExpression>
#include <algorithm>

namespace Series {

namespace {

const QRegularExpression &seasonEpisode() {
    static const QRegularExpression re(
        R"((?<![a-z0-9])S(\d{1,2})[ ._-]?E(\d{1,3})(?:[ ._-]?-?E(\d{1,3}))?(?![0-9]))",
        QRegularExpression::CaseInsensitiveOption);
    return re;
}

const QRegularExpression &crossEpisode() {
    static const QRegularExpression re(R"((?<![a-z0-9])(\d{1,2})x(\d{2,3})(?![0-9a-z]))",
                                       QRegularExpression::CaseInsensitiveOption);
    return re;
}

const QRegularExpression &releaseTag() {
    static const QRegularExpression re(
        R"((?<![a-z0-9])(2160p|1080p|720p|576p|480p|4k|uhd|hdr10?|web[ -]?dl|webrip|web|blu[ -]?ray|brrip|bdrip|hdtv|dvdrip|)"
        R"(x26[45]|h[ ]?26[45]|hevc|avc|aac|ac3|e-?ac3|dts|ddp?[ ]?[257][ ]?[01]|10bit|8bit|remux|proper|repack|amzn|nf|dsnp|hmax|atvp)(?![a-z0-9]))",
        QRegularExpression::CaseInsensitiveOption);
    return re;
}

// Kotlin tidy(): brackets to spaces, runs of spaces to one, ends trimmed of "-–· ".
QString tidy(const QString &s) {
    static const QRegularExpression brackets(R"([\[\](){}])");
    static const QRegularExpression spaces(R"(\s+)");
    QString out = QString(s).replace(brackets, " ").replace(spaces, " ").trimmed();
    const QString strip = QStringLiteral("-–· ");
    qsizetype from = 0, to = out.size();
    while (from < to && strip.contains(out.at(from)))
        ++from;
    while (to > from && strip.contains(out.at(to - 1)))
        --to;
    return out.mid(from, to - from);
}

QString spacedName(const QString &stem) {
    static const QRegularExpression dots("[._]+");
    static const QRegularExpression group(R"(^\s*\[[^\]]*\]\s*)");
    return QString(stem).replace(dots, " ").replace(group, "").trimmed();
}

QString code(const QString &season, const QString &episode, const QString &last) {
    QString c = QString("S%1E%2").arg(season.toInt(), 2, 10, QChar('0')).arg(episode.toInt(), 2, 10, QChar('0'));
    if (!last.isEmpty())
        c += QString("-E%1").arg(last.toInt(), 2, 10, QChar('0'));
    return c;
}

bool finished(const File &f) { return f.positionMs <= 0 && f.durationMs > 0 && f.watchedAt > 0; }

bool naturalLess(const File &a, const File &b) {
    const QString ka = Links::naturalMediaKey(a.path), kb = Links::naturalMediaKey(b.path);
    return ka != kb ? ka < kb : a.index < b.index;
}

} // namespace

QString episodeTitle(const QString &fileName) {
    const QString base = fileName.section('/', -1);
    const QString stem = Links::isVideo(base) && base.contains('.') ? base.left(base.lastIndexOf('.')) : base;
    const QString spaced = spacedName(stem);
    auto titled = [&](const QRegularExpressionMatch &m, const QString &c) {
        const QString show = tidy(spaced.left(m.capturedStart()));
        return show.isEmpty() ? c : c + " · " + show;
    };
    if (const auto m = seasonEpisode().match(spaced); m.hasMatch())
        return titled(m, code(m.captured(1), m.captured(2), m.captured(3)));
    if (const auto m = crossEpisode().match(spaced); m.hasMatch())
        return titled(m, code(m.captured(1), m.captured(2), {}));
    const auto tag = releaseTag().match(spaced);
    const QString cut = tidy(spaced.left(tag.hasMatch() ? tag.capturedStart() : spaced.size()));
    if (!cut.isEmpty())
        return cut;
    return stem.isEmpty() ? base : stem;
}

int seasonOf(const QString &path) {
    const QString spaced = QString(path).replace(QRegularExpression("[._]+"), " ");
    if (const auto m = seasonEpisode().match(spaced); m.hasMatch())
        return m.captured(1).toInt();
    if (const auto m = crossEpisode().match(spaced); m.hasMatch())
        return m.captured(1).toInt();
    static const QRegularExpression folder(R"((?<![a-z])(?:season[ -]?|S)(\d{1,2})(?![0-9a-z]))",
                                           QRegularExpression::CaseInsensitiveOption);
    if (const auto m = folder.match(spaced); m.hasMatch())
        return m.captured(1).toInt();
    return -1;
}

int continueIndex(const QList<File> &files) {
    QList<File> playable;
    for (const File &f : files)
        if (f.video && !Links::isExtraVideo(f.path) && (f.selected || f.verifiedComplete))
            playable.append(f);
    const File *last = nullptr;
    for (const File &f : playable)
        if (f.watchedAt > 0 && (!last || f.watchedAt > last->watchedAt))
            last = &f;
    if (!last)
        return -1;
    if (!finished(*last))
        return last->index;
    const File *next = nullptr;
    for (const File &f : playable)
        if (naturalLess(*last, f) && (!next || naturalLess(f, *next)))
            next = &f;
    return next ? next->index : -1;
}

int resumeFile(const QList<File> &files) {
    const File *best = nullptr;
    for (const File &f : files) {
        if (!f.video || f.positionMs <= 0 || (!f.selected && !f.verifiedComplete))
            continue;
        if (f.durationMs > 0 && f.positionMs >= f.durationMs - 3000)
            continue;
        if (!best || f.watchedAt > best->watchedAt || (f.watchedAt == best->watchedAt && f.index < best->index))
            best = &f;
    }
    return best ? best->index : -1;
}

int pickPlayIndex(const QList<File> &files) {
    QList<File> pool;
    for (const File &f : files)
        if (f.selected)
            pool.append(f);
    if (pool.isEmpty())
        for (const File &f : files)
            if (f.verifiedComplete)
                pool.append(f);
    QList<File> videos, all;
    for (const File &f : pool)
        if (f.video) {
            all.append(f);
            if (!Links::isExtraVideo(f.path))
                videos.append(f);
        }
    if (videos.isEmpty())
        videos = all;
    if (videos.size() > 1) {
        const File *first = nullptr;
        for (const File &f : videos)
            if (!first || Links::naturalMediaKey(f.path) < Links::naturalMediaKey(first->path))
                first = &f;
        return first->index;
    }
    const QList<File> &from = videos.isEmpty() ? pool : videos;
    const File *largest = nullptr;
    for (const File &f : from)
        if (!largest || f.length > largest->length)
            largest = &f;
    return largest ? largest->index : -1;
}

int resumeIndex(const QList<File> &files) {
    int index = continueIndex(files);
    if (index < 0)
        index = resumeFile(files);
    if (index < 0)
        index = pickPlayIndex(files);
    return index;
}

QString upNextLabel(const QJsonObject &preload, int fileIndex) {
    if (preload.isEmpty() || preload.value("fileIndex").toInt(-1) != fileIndex || fileIndex < 0)
        return {};
    if (preload.value("ready").toBool())
        return QObject::tr("Up next · start loaded");
    if (preload.value("active").toBool())
        return QObject::tr("Up next · loading start");
    return QObject::tr("Up next · loads near the end");
}

QString groupOf(const File &f) {
    if (f.video && Links::isExtraVideo(f.path))
        return "x";
    const int season = seasonOf(f.path.isEmpty() ? f.name : f.path);
    return season >= 0 ? "s" + QString::number(season) : QString();
}

QList<int> displayOrder(const QList<File> &files) {
    struct Item {
        int rank;
        int season;
        QString key;
        int index;
    };
    QList<Item> items;
    for (const File &f : files) {
        const QString group = groupOf(f);
        const int rank = group.startsWith('s') ? 0 : group.isEmpty() ? 1 : 2;
        items.append({rank, rank == 0 ? group.mid(1).toInt() : 0, Links::naturalMediaKey(f.name), f.index});
    }
    std::stable_sort(items.begin(), items.end(), [](const Item &a, const Item &b) {
        if (a.rank != b.rank)
            return a.rank < b.rank;
        if (a.season != b.season)
            return a.season < b.season;
        if (a.key != b.key)
            return a.key < b.key;
        return a.index < b.index;
    });
    QList<int> order;
    for (const Item &item : items)
        order.append(item.index);
    return order;
}

}
