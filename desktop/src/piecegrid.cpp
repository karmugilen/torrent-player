#include "piecegrid.h"

#include "engine.h"
#include "format.h"
#include "playerstats.h"

#include <QCoreApplication>
#include <QJsonArray>
#include <QMap>
#include <QMouseEvent>
#include <QPainter>
#include <QPainterPath>
#include <QtQml>
#include <cmath>

namespace PieceHeat {

int Summary::verifiedPercent() const {
    if (selected <= 0)
        return -1;
    return qBound(0, int((100.0 * selectedVerified) / selected), 100);
}

QString Summary::titlePrimary() const {
    if (selected <= 0)
        return QObject::tr("No selected pieces");
    return QObject::tr("%1% verified · %2 receiving").arg(verifiedPercent()).arg(selectedReceiving);
}

QString Summary::titleMeta() const {
    const QString pieces = QString::number(totalPieces);
    return pieceLength > 0 ? pieces + " · " + Format::bytes(double(pieceLength)) : pieces;
}

Bucket normalize(int index, int start, int end, int total, int selected, int selectedVerified, int selectedReceiving) {
    Bucket b;
    b.index = index;
    b.start = start;
    b.end = qMax(end, start);
    b.total = qMax(0, total);
    b.selected = qBound(0, selected, b.total);
    b.selectedVerified = qBound(0, selectedVerified, b.selected);
    b.selectedReceiving = qBound(0, selectedReceiving, b.selected - b.selectedVerified);
    b.missing = b.selected - b.selectedVerified - b.selectedReceiving;
    b.excluded = b.total - b.selected;
    return b;
}

Map parse(const QJsonObject &json) {
    Map map;
    map.id = json.value("id").toString();
    map.generation = json.value("generation").toInteger(1);
    map.totalPieces = json.value("totalPieces").toInt();
    map.pieceLength = json.value("pieceLength").toInteger();
    const QJsonArray buckets = json.value("buckets").toArray();
    for (int i = 0; i < buckets.size() && i < MaxCells; ++i) {
        const QJsonObject b = buckets.at(i).toObject();
        const int start = b.value("start").toInt();
        const int end = b.value("end").toInt(start);
        map.buckets.append(normalize(i, start, end, b.value("total").toInt(end - start + 1), b.value("selected").toInt(),
                                     b.value("selectedVerified").toInt(), b.value("selectedReceiving").toInt()));
    }
    for (const auto &value : json.value("playback").toArray()) {
        const QJsonObject r = value.toObject();
        const Range range{r.value("start").toInt(-1), r.value("end").toInt(-1), r.value("priority").toString()};
        if (range.start >= 0 && range.end >= range.start && playbackRank(range.priority) > 0)
            map.playback.append(range);
    }
    for (const auto &value : json.value("playheads").toArray())
        if (value.toInt(-1) >= 0)
            map.playheads.append(value.toInt());
    const QJsonObject detail = json.value("playbackDetail").toObject();
    const QString states = detail.value("states").toString();
    const int start = detail.value("start").toInt(-1);
    static const QRegularExpression valid("^[vsrm]+$");
    if (start >= 0 && valid.match(states).hasMatch()) {
        map.hasDetail = true;
        map.detailStart = start;
        map.detailStates = states;
        map.detailPlayhead = detail.value("playhead").toInt(start);
    }
    return map;
}

Summary summarize(const Map &map) {
    Summary s;
    for (const Bucket &b : map.buckets) {
        s.selected += b.selected;
        s.selectedVerified += b.selectedVerified;
        s.selectedReceiving += b.selectedReceiving;
        s.missing += b.missing;
        s.excluded += b.excluded;
    }
    s.totalPieces = qMax(0, map.totalPieces);
    s.pieceLength = qMax<qint64>(0, map.pieceLength);
    return s;
}

Layout layoutFor(int bucketCount) {
    const int count = qBound(0, bucketCount, MaxCells);
    Layout l;
    l.columns = count <= 0 ? 1 : count <= 64 ? 16 : count <= 128 ? 24 : 32;
    l.cellDp = count <= 64 ? 8 : count <= 128 ? 6.5 : 5;
    l.gapDp = count <= 64 ? 0.9 : count <= 128 ? 0.8 : 0.75;
    l.rows = count == 0 ? 0 : int(std::ceil(count / double(l.columns)));
    l.cellCount = count;
    return l;
}

double heightFor(const Layout &layout, double widthPx, double density) {
    if (layout.rows <= 0 || widthPx <= 0 || density <= 0)
        return layout.heightDp();
    const double widthDp = widthPx / density;
    const double gapsX = qMax(0, layout.columns - 1) * layout.gapDp;
    const double gapsY = qMax(0, layout.rows - 1) * layout.gapDp;
    const double cell = qMax(6.0, (widthDp - gapsX) / layout.columns);
    return layout.rows * cell + gapsY;
}

QPair<double, double> squareCell(double width, double, const Layout &layout) {
    if (layout.cellCount <= 0 || width <= 0 || layout.columns <= 0)
        return {0, 0};
    const int cols = layout.columns;
    const double unit = cols * layout.cellDp + (cols - 1) * layout.gapDp;
    const double scale = width / qMax(0.001, unit);
    const double gap = layout.gapDp * scale;
    const double cell = (width - gap * qMax(0, cols - 1)) / cols;
    return {qMax(0.0, cell), gap};
}

int hitTest(double x, double y, double width, double height, const Layout &layout) {
    if (layout.cellCount <= 0 || width <= 0 || height <= 0)
        return -1;
    if (x < 0 || y < 0 || x > width || y > height)
        return -1;
    const auto [cell, gap] = squareCell(width, height, layout);
    if (cell <= 0)
        return -1;
    const int col = int(x / (cell + gap));
    const int row = int(y / (cell + gap));
    if (col < 0 || col >= layout.columns || row < 0 || row >= layout.rows)
        return -1;
    if (x - col * (cell + gap) > cell || y - row * (cell + gap) > cell)
        return -1;
    const int index = row * layout.columns + col;
    return index < layout.cellCount ? index : -1;
}

QString shortFileLabel(const QString &name, int maxLength) {
    QString base = name.section('/', -1).section('\\', -1);
    if (base.trimmed().isEmpty())
        base = name;
    if (base.size() <= maxLength)
        return base;
    return base.left(maxLength - 1) + QStringLiteral("…");
}

QList<Marker> fileBoundaryMarkers(const QList<FileSpan> &files, qint64 pieceLength, const QList<Bucket> &buckets,
                                  int maxLabels) {
    if (files.isEmpty() || pieceLength <= 0 || buckets.isEmpty())
        return {};
    QList<FileSpan> ordered = files;
    std::stable_sort(ordered.begin(), ordered.end(), [](const FileSpan &a, const FileSpan &b) { return a.index < b.index; });
    // Names grouped by the piece each file starts in, in first-seen order.
    QList<QPair<int, QStringList>> starts;
    qint64 offset = 0;
    for (const FileSpan &f : ordered) {
        const int piece = int(offset / pieceLength);
        auto it = std::find_if(starts.begin(), starts.end(), [piece](const auto &s) { return s.first == piece; });
        if (it == starts.end())
            starts.append(qMakePair(piece, QStringList{shortFileLabel(f.name)}));
        else
            it->second.append(shortFileLabel(f.name));
        offset += qMax<qint64>(0, f.length);
    }
    QList<QPair<int, QStringList>> byBucket;
    for (const auto &[piece, names] : starts) {
        int bucket = -1;
        for (int i = 0; i < buckets.size(); ++i)
            if (piece >= buckets[i].start && piece <= buckets[i].end) {
                bucket = i;
                break;
            }
        if (bucket < 0)
            continue;
        auto it = std::find_if(byBucket.begin(), byBucket.end(), [bucket](const auto &b) { return b.first == bucket; });
        if (it == byBucket.end())
            byBucket.append(qMakePair(bucket, names));
        else
            it->second += names;
    }
    QList<Marker> markers;
    for (int order = 0; order < byBucket.size(); ++order) {
        QStringList names = byBucket[order].second;
        names.removeDuplicates();
        Marker m{byBucket[order].first, {}, names.size() > 1};
        if (order < qMax(0, maxLabels)) {
            if (names.size() > 1)
                m.label = names.mid(0, 2).join(" / ") + (names.size() > 2 ? " +" : "") + QObject::tr(" (shared)");
            else if (!names.isEmpty())
                m.label = names.first();
        }
        markers.append(m);
    }
    return markers;
}

QString peekLine(const Bucket &b, bool memoryBuffer) {
    QString line = QObject::tr("Bucket %1 · pieces %2–%3 · %4 %5 · %6 receiving · %7 missing")
                       .arg(b.index)
                       .arg(b.start)
                       .arg(b.end)
                       .arg(b.selectedVerified)
                       .arg(memoryBuffer ? QObject::tr("buffered") : QObject::tr("verified"))
                       .arg(b.selectedReceiving)
                       .arg(b.missing);
    if (b.excluded > 0)
        line += QObject::tr(" · %1 excluded").arg(b.excluded);
    return line;
}

QString accessibleSummary(const Summary &s, bool memoryBuffer) {
    if (memoryBuffer)
        return QObject::tr("RAM pieces map. %1 buffered, %2 receiving, %3 empty, %4 excluded. Evicted pieces become empty.")
            .arg(s.selectedVerified)
            .arg(s.selectedReceiving)
            .arg(s.missing)
            .arg(s.excluded);
    const int pct = s.verifiedPercent();
    const QString head = pct < 0 ? QObject::tr("Pieces map. No selected pieces.")
                                 : QObject::tr("Pieces map. %1 percent verified. %2 receiving.").arg(pct).arg(s.selectedReceiving);
    QString text = head + " " +
                   QObject::tr("%1 verified, %2 receiving, %3 missing, %4 excluded. %5 pieces")
                       .arg(s.selectedVerified)
                       .arg(s.selectedReceiving)
                       .arg(s.missing)
                       .arg(s.excluded)
                       .arg(s.totalPieces);
    text += s.pieceLength > 0 ? QObject::tr(", %1 each.").arg(Format::bytes(double(s.pieceLength))) : QStringLiteral(".");
    return text;
}

int playbackRank(const QString &p) {
    if (p == "U") return 5;
    if (p == "N") return 4;
    if (p == "R") return 3;
    if (p == "L") return 2;
    if (p == "B") return 1;
    return 0;
}

QString playbackLabel(const QString &p) {
    if (p == "U") return QObject::tr("this block");
    if (p == "N") return QObject::tr("next");
    if (p == "R") return QObject::tr("readahead");
    if (p == "L") return QObject::tr("lookahead");
    return QObject::tr("rewind");
}

QList<Band> playbackBands(const Bucket &bucket, const QList<Range> &ranges) {
    if (bucket.excludedOnly() || bucket.total <= 0)
        return {};
    const qint64 start = bucket.start, end = qint64(bucket.end) + 1;
    if (end <= start)
        return {};
    struct Clip {
        qint64 begin, finish;
        QString priority;
    };
    QList<Clip> clipped;
    QList<qint64> edges;
    for (const Range &r : ranges) {
        const qint64 begin = qMax(start, qint64(r.start));
        const qint64 finish = qMin(end, qint64(r.end) + 1);
        if (begin < finish && playbackRank(r.priority) > 0) {
            clipped.append({begin, finish, r.priority});
            edges << begin << finish;
        }
    }
    std::sort(edges.begin(), edges.end());
    edges.erase(std::unique(edges.begin(), edges.end()), edges.end());
    QList<Band> bands;
    for (int i = 0; i + 1 < edges.size(); ++i) {
        const qint64 begin = edges[i], finish = edges[i + 1];
        QString best;
        for (const Clip &c : clipped)
            if (c.begin <= begin && c.finish >= finish && (best.isEmpty() || playbackRank(c.priority) > playbackRank(best)))
                best = c.priority;
        if (best.isEmpty())
            continue;
        bands.append({best, double(begin - start) / double(end - start), double(finish - begin) / double(end - start),
                      int(finish - begin)});
    }
    return bands;
}

static QString dominant(const QMap<QString, int> &counts) {
    QString best;
    int bestCount = -1;
    for (auto it = counts.cbegin(); it != counts.cend(); ++it)
        if (it.value() > bestCount || (it.value() == bestCount && playbackRank(it.key()) > playbackRank(best))) {
            best = it.key();
            bestCount = it.value();
        }
    return best;
}

QString dominantTier(const QList<Band> &bands) {
    QMap<QString, int> counts;
    for (const Band &b : bands)
        counts[b.priority] += b.pieces;
    return dominant(counts);
}

QString tierAt(int piece, const QList<Range> &ranges) {
    QString best;
    for (const Range &r : ranges)
        if (piece >= r.start && piece <= r.end && (best.isEmpty() || playbackRank(r.priority) > playbackRank(best)))
            best = r.priority;
    return best;
}

BufferModel bufferModel(int start, const QString &states, int playhead, const QList<Range> &ranges, int maxTiles) {
    const int n = states.size();
    QStringList tiers;
    for (int i = 0; i < n; ++i)
        tiers.append(tierAt(start + i, ranges));
    auto ready = [&](int i) { return states[i] == 'v' || states[i] == 's'; };
    const int head = playhead - start;
    BufferModel model;
    if (head >= 0 && head < n)
        while (head + model.readyAhead < n && ready(head + model.readyAhead))
            ++model.readyAhead;
    while (head - model.readyBehind - 1 >= 0 && head - model.readyBehind - 1 < n && ready(head - model.readyBehind - 1))
        ++model.readyBehind;
    const int per = qMax(1, int(std::ceil(n / double(qMax(1, maxTiles)))));
    model.piecesPerTile = per;
    for (int first = 0; first < n; first += per) {
        const int last = qMin(n, first + per) - 1;
        QMap<QString, int> counts;
        Tile tile;
        tile.start = start + first;
        tile.end = start + last;
        for (int i = first; i <= last; ++i) {
            if (!tiers[i].isEmpty())
                counts[tiers[i]] += 1;
            tile.verified += states[i] == 'v';
            tile.saved += states[i] == 's';
            tile.receiving += states[i] == 'r';
            ++tile.total;
        }
        tile.tier = dominant(counts);
        tile.playhead = head >= first && head <= last;
        model.tiles.append(tile);
    }
    for (const QString tier : {"U", "N", "R", "L", "B"}) {
        TierCount count{tier, 0, 0};
        for (int i = 0; i < n; ++i)
            if (tiers[i] == tier) {
                ++count.total;
                count.ready += ready(i);
            }
        if (count.total > 0)
            model.tiers.append(count);
    }
    return model;
}

}

// ---------------------------------------------------------------- item

using namespace PieceHeat;

static void registerTypes() {
    qmlRegisterType<PieceGrid>("TorrentPlayer", 1, 0, "PieceGrid");
    qmlRegisterType<PlayerStats>("TorrentPlayer", 1, 0, "PlayerStats");
}
Q_COREAPP_STARTUP_FUNCTION(registerTypes)

PieceGrid::PieceGrid(QQuickItem *parent) : QQuickPaintedItem(parent) {
    setAcceptedMouseButtons(Qt::LeftButton);
    setAntialiasing(true);
    m_poll.setInterval(1000);
    connect(&m_poll, &QTimer::timeout, this, &PieceGrid::refresh);
    m_animation.setInterval(50);
    connect(&m_animation, &QTimer::timeout, this, [this] { update(); });
    connect(this, &QQuickItem::visibleChanged, this, &PieceGrid::updatePolling);
    m_clock.start();
    rebuild();
}

QObject *PieceGrid::engineObject() const { return m_engine; }

void PieceGrid::setEngineObject(QObject *engine) {
    auto *typed = qobject_cast<Engine *>(engine);
    if (typed == m_engine)
        return;
    m_engine = typed;
    emit engineChanged();
    updatePolling();
}

void PieceGrid::setTorrentId(const QString &id) {
    if (id == m_id)
        return;
    m_id = id;
    // A new engine id must not keep a stale map.
    m_map = {};
    m_peekIndex = -1;
    m_peek.clear();
    emit peekChanged();
    rebuild();
    emit torrentIdChanged();
    updatePolling();
}

void PieceGrid::setActive(bool active) {
    if (active == m_active)
        return;
    m_active = active;
    emit activeChanged();
    updatePolling();
}

void PieceGrid::setFrozen(bool frozen) {
    if (frozen == m_frozen)
        return;
    m_frozen = frozen;
    emit frozenChanged();
    rebuild();
}

void PieceGrid::setMemoryBuffer(bool on) {
    if (on == m_memoryBuffer)
        return;
    m_memoryBuffer = on;
    emit memoryBufferChanged();
    rebuild();
}

void PieceGrid::setFiles(const QVariantList &files) {
    if (files == m_files)
        return;
    m_files = files;
    emit filesChanged();
    rebuild();
}

#define COLOR_SETTER(name, member)                                                                                     \
    void PieceGrid::name(const QColor &c) {                                                                            \
        if (c == member)                                                                                               \
            return;                                                                                                    \
        member = c;                                                                                                    \
        emit colorsChanged();                                                                                          \
        update();                                                                                                      \
    }
COLOR_SETTER(setDoneColor, m_done)
COLOR_SETTER(setOutlineColor, m_outline)
COLOR_SETTER(setSurfaceColor, m_surface)
COLOR_SETTER(setOnSurfaceColor, m_onSurface)
#undef COLOR_SETTER

// Android DefaultPieceColors.
QColor PieceGrid::gettingColor() const { return QColor(0xFF, 0xB3, 0x00); }

QColor PieceGrid::tierColor(const QString &p) {
    if (p == "U") return QColor(0xFF, 0x8A, 0x80);
    if (p == "N") return QColor(0xF4, 0x8F, 0xB1);
    if (p == "R") return QColor(0x82, 0xB1, 0xFF);
    if (p == "L") return QColor(0xCE, 0x93, 0xD8);
    return QColor(0x90, 0xA4, 0xAE);
}

void PieceGrid::setPieces(const QVariantMap &json) { apply(QJsonObject::fromVariantMap(json)); }

void PieceGrid::updatePolling() {
    const bool on = m_active && isVisible() && m_engine && !m_id.isEmpty();
    if (on && !m_poll.isActive()) {
        m_poll.start();
        refresh(); // first map promptly
    } else if (!on) {
        m_poll.stop();
    }
    rebuild();
}

void PieceGrid::refresh() {
    if (!m_engine || m_id.isEmpty() || m_busy)
        return;
    m_busy = true;
    const QString id = m_id;
    QPointer<PieceGrid> self(this);
    m_engine->get(QString("/pieces/%1?maxBuckets=%2").arg(id).arg(MaxCells), [self, id](const QJsonObject &json, int code) {
        if (!self)
            return;
        self->m_busy = false;
        if (code == 200 && id == self->m_id)
            self->apply(json);
    });
}

void PieceGrid::apply(const QJsonObject &json) {
    m_map = parse(json);
    rebuild();
}

void PieceGrid::rebuild() {
    m_sum = summarize(m_map);
    m_layout = layoutFor(m_map.buckets.size());
    m_bands.clear();
    for (const Bucket &b : m_map.buckets)
        m_bands.append(playbackBands(b, m_map.playback));
    m_playheadCells.clear();
    for (int head : m_map.playheads)
        for (const Bucket &b : m_map.buckets)
            if (head >= b.start && head <= b.end) {
                if (!m_playheadCells.contains(b.index))
                    m_playheadCells.append(b.index);
                break;
            }
    QList<FileSpan> spans;
    for (const QVariant &v : m_files) {
        const QVariantMap f = v.toMap();
        spans.append({f.value("index").toInt(), f.value("name").toString(), qint64(f.value("length").toDouble())});
    }
    m_markers = fileBoundaryMarkers(spans, m_map.pieceLength, m_map.buckets);

    QString bufferLine;
    if (m_map.hasDetail) {
        const BufferModel buffer = bufferModel(m_map.detailStart, m_map.detailStates, m_map.detailPlayhead, m_map.playback);
        bufferLine = tr("Ready ahead %1 · rewind %2")
                         .arg(Format::bytes(double(buffer.readyAhead * m_map.pieceLength)),
                              Format::bytes(double(buffer.readyBehind * m_map.pieceLength)));
    }
    bool playback = false;
    for (const auto &bands : std::as_const(m_bands))
        playback = playback || !bands.isEmpty();
    QString accessible = accessibleSummary(m_sum, m_memoryBuffer);
    if (m_frozen)
        accessible += tr(" Map paused.");
    if (playback)
        accessible += tr(" Playback windows are tinted by tier; the ringed cell holds the playhead. Tinted outline means "
                         "waiting, solid means downloaded.");
    m_summary = {{"verifiedPercent", m_sum.verifiedPercent()},
                 {"titlePrimary", m_sum.titlePrimary()},
                 {"titleMeta", m_sum.titleMeta()},
                 {"done", m_sum.selectedVerified},
                 {"getting", m_sum.selectedReceiving},
                 {"waiting", m_sum.missing},
                 {"off", m_sum.excluded},
                 {"accessible", accessible},
                 {"bufferLine", bufferLine}};
    if (m_peekIndex >= m_map.buckets.size())
        m_peekIndex = -1;
    if (m_peekIndex >= 0) {
        QString line = peekLine(m_map.buckets[m_peekIndex], m_memoryBuffer);
        QMap<QString, int> tiers;
        for (const Band &b : m_bands[m_peekIndex])
            tiers[b.priority] += b.pieces;
        for (auto it = tiers.cbegin(); it != tiers.cend(); ++it)
            line += " · " + playbackLabel(it.key()) + " " + QString::number(it.value());
        if (line != m_peek) {
            m_peek = line;
            emit peekChanged();
        }
    }
    const bool breathe = m_sum.selectedReceiving > 0 && !m_frozen && isVisible();
    if (breathe && !m_animation.isActive())
        m_animation.start();
    else if (!breathe)
        m_animation.stop();
    updateHeight();
    emit mapChanged();
    update();
}

void PieceGrid::updateHeight() { setImplicitHeight(heightFor(m_layout, width())); }

void PieceGrid::geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) {
    QQuickPaintedItem::geometryChange(newGeometry, oldGeometry);
    if (newGeometry.width() != oldGeometry.width())
        updateHeight();
}

// Receiving blocks breathe between 0.55 and 1 every 700 ms, as on Android.
double PieceGrid::pulse() const {
    if (!m_animation.isActive())
        return 1;
    const double t = std::fmod(m_clock.elapsed() / 700.0, 2.0);
    const double tri = t < 1 ? t : 2 - t;
    return 0.55 + 0.45 * tri;
}

static QColor alpha(QColor c, double a) {
    c.setAlphaF(qBound(0.0, c.alphaF() * a, 1.0));
    return c;
}

void PieceGrid::paint(QPainter *p) {
    if (m_map.buckets.isEmpty())
        return;
    const auto [cell, gap] = squareCell(width(), height(), m_layout);
    if (cell <= 0)
        return;
    p->setRenderHint(QPainter::Antialiasing);
    const int cols = m_layout.columns;
    const double radius = qBound(2.5, cell * 0.28, 6.0);
    const QColor receiving = gettingColor();
    const QColor receivingGlow(0xFF, 0x6F, 0x00);
    const QColor missingStroke = alpha(m_outline, 0.35);
    const QColor excludedStroke = alpha(m_outline, 0.55);
    const QColor excludedFill = alpha(m_surface, 0.25);
    const double breathing = pulse();
    auto rectAt = [&](int index) {
        return QRectF((index % cols) * (cell + gap), (index / cols) * (cell + gap), cell, cell);
    };
    auto fill = [&](const QRectF &r, const QColor &c) {
        p->setPen(Qt::NoPen);
        p->setBrush(c);
        p->drawRoundedRect(r, radius, radius);
    };
    auto stroke = [&](const QRectF &r, const QColor &c, double w) {
        p->setPen(QPen(c, w));
        p->setBrush(Qt::NoBrush);
        const double h = w / 2;
        p->drawRoundedRect(r.adjusted(h, h, -h, -h), radius, radius);
    };

    for (int i = 0; i < m_map.buckets.size(); ++i) {
        const Bucket &b = m_map.buckets[i];
        const QRectF r = rectAt(i);
        if (b.excludedOnly()) {
            fill(r, excludedFill);
            stroke(r, excludedStroke, 1.1);
            continue;
        }
        const double progress = b.selected > 0 ? double(b.selectedVerified) / b.selected : 0;
        const bool working = b.selectedReceiving > 0;
        const QString priority = dominantTier(m_bands.value(i));
        const QColor tint = priority.isEmpty() ? m_done : tierColor(priority);
        if (working)
            fill(r, alpha(receiving, breathing));
        else if (progress >= 1)
            fill(r, tint);
        else if (progress > 0) // partial blocks deepen toward the done colour
            fill(r, alpha(tint, 0.22 + 0.6 * progress));
        else if (!priority.isEmpty())
            fill(r, alpha(tint, 0.16));
        if (working)
            stroke(r, priority.isEmpty() ? receivingGlow : tint, 1.8);
        else if (progress < 1)
            stroke(r, priority.isEmpty() ? missingStroke : alpha(tint, 0.85), priority.isEmpty() ? 1.1 : 1.8);
    }

    for (int index : std::as_const(m_playheadCells)) {
        const double ring = qMin(gap, 2.0);
        p->setPen(QPen(m_onSurface, 2.2));
        p->setBrush(Qt::NoBrush);
        p->drawRoundedRect(rectAt(index).adjusted(-ring, -ring, ring, ring), radius, radius);
    }

    QPen tick(alpha(m_onSurface, 0.75), 1.6);
    tick.setCapStyle(Qt::RoundCap);
    p->setPen(tick);
    for (const Marker &m : std::as_const(m_markers)) {
        const QRectF r = rectAt(m.bucketIndex);
        const double y = r.bottom() + qMin(gap * 0.4, 2.5);
        p->drawLine(QPointF(r.left() + cell * 0.2, y), QPointF(r.left() + cell * 0.8, y));
    }
}

void PieceGrid::mousePressEvent(QMouseEvent *event) {
    const int hit = hitTest(event->position().x(), event->position().y(), width(), height(), m_layout);
    m_peekIndex = hit;
    if (hit < 0 && !m_peek.isEmpty()) {
        m_peek.clear();
        emit peekChanged();
    }
    rebuild();
    event->accept();
}
