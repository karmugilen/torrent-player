#pragma once

#include <QColor>
#include <QElapsedTimer>
#include <QJsonObject>
#include <QList>
#include <QPair>
#include <QPointer>
#include <QQuickPaintedItem>
#include <QString>
#include <QTimer>
#include <QVariantList>
#include <QVariantMap>

class Engine;

// The piece heat grid's model (Android PieceHeatGridModel.kt). Pure, tested.
namespace PieceHeat {

constexpr int MaxCells = 256;
constexpr int MaxPlaybackTiles = 512;

// Disjoint selected-aware counts for one inclusive piece bucket.
struct Bucket {
    int index = 0, start = 0, end = 0, total = 0, selected = 0;
    int selectedVerified = 0, selectedReceiving = 0, missing = 0, excluded = 0;

    double verifiedFraction() const { return fraction(selectedVerified); }
    double receivingFraction() const { return fraction(selectedReceiving); }
    double missingFraction() const { return fraction(missing); }
    double excludedFraction() const { return fraction(excluded); }
    // No selected pieces: a hollow tile.
    bool excludedOnly() const { return selected == 0 && total > 0; }
    // Selected work remains or excluded pieces share the cell; never painted as done.
    bool mixedFill() const {
        return total > 0 && !excludedOnly() &&
               (selectedReceiving > 0 || missing > 0 || excluded > 0 || selectedVerified < selected);
    }

private:
    double fraction(int count) const { return total <= 0 ? 0 : qBound(0.0, double(count) / total, 1.0); }
};

struct Summary {
    int totalPieces = 0;
    qint64 pieceLength = 0;
    int selected = 0, selectedVerified = 0, selectedReceiving = 0, missing = 0, excluded = 0;
    // Selected verified percent, -1 when nothing is selected.
    int verifiedPercent() const;
    QString titlePrimary() const;
    QString titleMeta() const;
};

struct Layout {
    int columns = 1, rows = 0, cellCount = 0;
    double cellDp = 8, gapDp = 0.9;
    double heightDp() const { return rows <= 0 ? 0 : rows * cellDp + qMax(0, rows - 1) * gapDp; }
};

struct Marker {
    int bucketIndex = 0;
    QString label; // null: tick only
    bool shared = false;
};

struct FileSpan {
    int index = 0;
    QString name;
    qint64 length = 0;
};

struct Range {
    int start = 0, end = 0;
    QString priority; // U, N, R, L or B
};

struct Band {
    QString priority;
    double startFraction = 0, fraction = 0;
    int pieces = 0;
};

struct Tile {
    int start = 0, end = 0;
    QString tier; // empty: outside every window
    int verified = 0, saved = 0, receiving = 0, total = 0;
    bool playhead = false;
    int ready() const { return verified + saved; }
    bool done() const { return total > 0 && ready() >= total; }
    double readyFraction() const { return total <= 0 ? 0 : double(ready()) / total; }
    bool savedOnly() const { return done() && saved > 0 && verified == 0; }
};

struct TierCount {
    QString tier;
    int ready = 0, total = 0;
};

struct BufferModel {
    QList<Tile> tiles;
    int piecesPerTile = 1;
    int readyAhead = 0, readyBehind = 0;
    QList<TierCount> tiers;
};

// One /pieces/<id> reply.
struct Map {
    QString id;
    qint64 generation = 0;
    int totalPieces = 0;
    qint64 pieceLength = 0;
    QList<Bucket> buckets; // normalized, at most MaxCells
    QList<Range> playback;
    QList<int> playheads;
    bool hasDetail = false;
    int detailStart = 0, detailPlayhead = 0;
    QString detailStates; // one of v s r m per piece
};

Bucket normalize(int index, int start, int end, int total, int selected, int selectedVerified, int selectedReceiving);
Map parse(const QJsonObject &json);
Summary summarize(const Map &map);
Layout layoutFor(int bucketCount);
// Height that keeps square cells filling the width (pieceHeatHeightDp).
double heightFor(const Layout &layout, double widthPx, double density = 1);
// Cell edge and gap that fill width exactly (pieceHeatSquareCell).
QPair<double, double> squareCell(double width, double height, const Layout &layout);
// Bucket under a point, or -1.
int hitTest(double x, double y, double width, double height, const Layout &layout);
QList<Marker> fileBoundaryMarkers(const QList<FileSpan> &files, qint64 pieceLength, const QList<Bucket> &buckets,
                                  int maxLabels = 3);
QString peekLine(const Bucket &bucket, bool memoryBuffer = false);
QString accessibleSummary(const Summary &summary, bool memoryBuffer = false);
QString shortFileLabel(const QString &name, int maxLength = 10);
QList<Band> playbackBands(const Bucket &bucket, const QList<Range> &ranges);
int playbackRank(const QString &priority);
QString dominantTier(const QList<Band> &bands);
QString tierAt(int piece, const QList<Range> &ranges);
BufferModel bufferModel(int start, const QString &states, int playhead, const QList<Range> &ranges,
                        int maxTiles = MaxPlaybackTiles);
QString playbackLabel(const QString &priority);

}

// The heat grid as Android draws it (PieceHeatGrid.kt): one rounded tile per bucket,
// teal/accent when done, amber while receiving (breathing), outlined while waiting,
// hollow when the file is skipped, tinted by playback window, the playhead ringed and
// file starts ticked. Polls /pieces/<id>?maxBuckets=256 once a second while visible.
class PieceGrid : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(QObject *engine READ engineObject WRITE setEngineObject NOTIFY engineChanged)
    Q_PROPERTY(QString torrentId READ torrentId WRITE setTorrentId NOTIFY torrentIdChanged)
    // Polling on (the page or panel is showing).
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY activeChanged)
    // Paused or complete: no breathing.
    Q_PROPERTY(bool frozen READ frozen WRITE setFrozen NOTIFY frozenChanged)
    // Direct Play: buffered pieces, not verified downloads.
    Q_PROPERTY(bool memoryBuffer READ memoryBuffer WRITE setMemoryBuffer NOTIFY memoryBufferChanged)
    // [{index, name, length}] for the file-start ticks.
    Q_PROPERTY(QVariantList files READ files WRITE setFiles NOTIFY filesChanged)
    Q_PROPERTY(QColor doneColor READ doneColor WRITE setDoneColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor outlineColor READ outlineColor WRITE setOutlineColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor surfaceColor READ surfaceColor WRITE setSurfaceColor NOTIFY colorsChanged)
    Q_PROPERTY(QColor onSurfaceColor READ onSurfaceColor WRITE setOnSurfaceColor NOTIFY colorsChanged)
    Q_PROPERTY(bool hasMap READ hasMap NOTIFY mapChanged)
    // {verifiedPercent (-1 none), titlePrimary, titleMeta, done, getting, waiting, off, accessible,
    //  bufferLine ("Ready ahead 4 MB · rewind 1 MB" or "")}
    Q_PROPERTY(QVariantMap summary READ summary NOTIFY mapChanged)
    // Line for the clicked tile, or "".
    Q_PROPERTY(QString peekText READ peekText NOTIFY peekChanged)
    // The Android colours, for legends.
    Q_PROPERTY(QColor gettingColor READ gettingColor CONSTANT)

public:
    explicit PieceGrid(QQuickItem *parent = nullptr);

    QObject *engineObject() const;
    void setEngineObject(QObject *engine);
    QString torrentId() const { return m_id; }
    void setTorrentId(const QString &id);
    bool active() const { return m_active; }
    void setActive(bool active);
    bool frozen() const { return m_frozen; }
    void setFrozen(bool frozen);
    bool memoryBuffer() const { return m_memoryBuffer; }
    void setMemoryBuffer(bool on);
    QVariantList files() const { return m_files; }
    void setFiles(const QVariantList &files);
    QColor doneColor() const { return m_done; }
    void setDoneColor(const QColor &c);
    QColor outlineColor() const { return m_outline; }
    void setOutlineColor(const QColor &c);
    QColor surfaceColor() const { return m_surface; }
    void setSurfaceColor(const QColor &c);
    QColor onSurfaceColor() const { return m_onSurface; }
    void setOnSurfaceColor(const QColor &c);
    QColor gettingColor() const;
    bool hasMap() const { return !m_map.buckets.isEmpty(); }
    QVariantMap summary() const { return m_summary; }
    QString peekText() const { return m_peek; }

    // A /pieces reply, as the poll gets it (also for tests).
    Q_INVOKABLE void setPieces(const QVariantMap &json);
    Q_INVOKABLE void refresh();

    void paint(QPainter *painter) override;

    // Android piece colours by playback tier.
    static QColor tierColor(const QString &priority);

signals:
    void engineChanged();
    void torrentIdChanged();
    void activeChanged();
    void frozenChanged();
    void memoryBufferChanged();
    void filesChanged();
    void colorsChanged();
    void mapChanged();
    void peekChanged();

protected:
    void mousePressEvent(QMouseEvent *event) override;
    void geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) override;

private:
    void apply(const QJsonObject &json);
    void rebuild();
    void updatePolling();
    void updateHeight();
    double pulse() const;

    QPointer<Engine> m_engine;
    QString m_id;
    bool m_active = true;
    bool m_frozen = false;
    bool m_memoryBuffer = false;
    bool m_busy = false;
    QVariantList m_files;
    QColor m_done{0xFF, 0xD6, 0x0A};
    QColor m_outline{0x93, 0x8F, 0x99};
    QColor m_surface{0x2A, 0x2A, 0x2F};
    QColor m_onSurface{0xEC, 0xEC, 0xEF};
    PieceHeat::Map m_map;
    PieceHeat::Summary m_sum;
    PieceHeat::Layout m_layout;
    QList<QList<PieceHeat::Band>> m_bands;
    QList<int> m_playheadCells;
    QList<PieceHeat::Marker> m_markers;
    QVariantMap m_summary;
    int m_peekIndex = -1;
    QString m_peek;
    QTimer m_poll;
    QTimer m_animation;
    QElapsedTimer m_clock;
};
