#include "worldmap.h"

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLineF>
#include <QMouseEvent>
#include <QPainter>
#include <QPainterPath>
#include <QtMath>
#include <limits>

WorldMap::WorldMap(QQuickItem *parent) : QQuickPaintedItem(parent) {
    setAcceptedMouseButtons(Qt::LeftButton);
    setAntialiasing(true);
    setOpaquePainting(true);
    rebuildSummary();
}

void WorldMap::setCountries(const QVariantList &countries) {
    if (m_countries == countries)
        return;
    m_countries = countries;
    rebuildSummary();
    emit countriesChanged();
    update();
}

void WorldMap::setAccent(const QColor &color) {
    if (m_accent == color)
        return;
    m_accent = color;
    emit colorsChanged();
    update();
}

void WorldMap::setLand(const QColor &color) {
    if (m_land == color)
        return;
    m_land = color;
    emit colorsChanged();
    update();
}

void WorldMap::setLandActive(const QColor &color) {
    if (m_landActive == color)
        return;
    m_landActive = color;
    emit colorsChanged();
    update();
}

void WorldMap::setGrid(const QColor &color) {
    if (m_grid == color)
        return;
    m_grid = color;
    emit colorsChanged();
    update();
}

void WorldMap::setBackground(const QColor &color) {
    if (m_background == color)
        return;
    m_background = color;
    emit colorsChanged();
    update();
}

void WorldMap::setSelectedCode(const QString &code) {
    if (m_selectedCode == code)
        return;
    m_selectedCode = code;
    rebuildSummary();
    emit selectedCodeChanged();
    update();
}

void WorldMap::geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) {
    QQuickPaintedItem::geometryChange(newGeometry, oldGeometry);
    update();
}

QPointF WorldMap::project(double longitude, double latitude, const QSizeF &size) const {
    return { (longitude + 180.0) / 360.0 * size.width(), (90.0 - latitude) / 180.0 * size.height() };
}

QHash<QString, int> WorldMap::countByCode() const {
    QHash<QString, int> counts;
    for (const QVariant &value : m_countries) {
        const QVariantMap row = value.toMap();
        const QString code = row.value(QStringLiteral("code")).toString().toUpper();
        if (!code.isEmpty())
            counts.insert(code, row.value(QStringLiteral("count")).toInt());
    }
    return counts;
}

void WorldMap::ensureGeometry() {
    if (m_geometryLoaded || m_geometryFailed)
        return;
    QFile file(QStringLiteral(":/world-map.json"));
    if (!file.open(QIODevice::ReadOnly)) {
        m_geometryFailed = true;
        rebuildSummary();
        return;
    }
    const QJsonArray array = QJsonDocument::fromJson(file.readAll()).array();
    m_shapes.clear();
    m_shapes.reserve(array.size());
    for (const QJsonValue &value : array) {
        const QJsonObject country = value.toObject();
        CountryShape shape;
        shape.code = country.value(QStringLiteral("code")).toString().toUpper();
        shape.name = country.value(QStringLiteral("name")).toString();
        shape.longitude = country.value(QStringLiteral("longitude")).toDouble();
        shape.latitude = country.value(QStringLiteral("latitude")).toDouble();
        const QJsonArray rings = country.value(QStringLiteral("rings")).toArray();
        for (const QJsonValue &ringValue : rings) {
            Ring ring;
            const QJsonArray points = ringValue.toArray();
            ring.points.reserve(points.size());
            for (const QJsonValue &pointValue : points) {
                const QJsonArray point = pointValue.toArray();
                if (point.size() < 2)
                    continue;
                ring.points.append(QPointF(point.at(0).toDouble(), point.at(1).toDouble()));
            }
            if (!ring.points.isEmpty())
                shape.rings.append(ring);
        }
        if (!shape.code.isEmpty())
            m_shapes.append(shape);
    }
    m_geometryLoaded = true;
    rebuildSummary();
}

void WorldMap::rebuildSummary() {
    ensureGeometry();
    const QHash<QString, int> counts = countByCode();
    int located = 0;
    int mappedCountries = 0;
    QString selectedLine;
    for (const CountryShape &shape : m_shapes) {
        const int count = counts.value(shape.code);
        if (count <= 0)
            continue;
        located += count;
        ++mappedCountries;
        if (shape.code == m_selectedCode)
            selectedLine = tr("%1 · %n connected peers", "", count).arg(shape.name);
    }
    if (m_geometryFailed) {
        m_summary = tr("World map unavailable");
    } else if (!m_geometryLoaded) {
        m_summary = tr("Loading world map…");
    } else if (!selectedLine.isEmpty()) {
        m_summary = selectedLine;
    } else if (mappedCountries > 0) {
        m_summary = tr("%1 countries · %2 located peers")
                        .arg(mappedCountries)
                        .arg(located);
    } else {
        m_summary = tr("No peer locations available");
    }
    emit summaryChanged();
}

void WorldMap::paint(QPainter *painter) {
    ensureGeometry();
    const QSizeF size = boundingRect().size();
    if (size.width() < 1 || size.height() < 1)
        return;

    painter->fillRect(boundingRect(), m_background);

    QColor grid = m_grid;
    grid.setAlpha(90);
    for (int longitude = -120; longitude <= 120; longitude += 60) {
        const qreal x = project(longitude, 0, size).x();
        painter->setPen(QPen(grid, 1));
        painter->drawLine(QPointF(x, 0), QPointF(x, size.height()));
    }
    for (int latitude = -60; latitude <= 60; latitude += 30) {
        const qreal y = project(0, latitude, size).y();
        painter->setPen(QPen(grid, 1));
        painter->drawLine(QPointF(0, y), QPointF(size.width(), y));
    }

    const QHash<QString, int> counts = countByCode();
    QColor activeFill = m_accent;
    activeFill.setAlpha(46);
    QColor idleFill = m_land;
    QColor activeStroke = m_accent;
    activeStroke.setAlpha(102);
    QColor idleStroke = m_accent;
    idleStroke.setAlpha(30);

    for (const CountryShape &shape : m_shapes) {
        const bool active = counts.value(shape.code) > 0;
        for (const Ring &ring : shape.rings) {
            QPainterPath path;
            for (int i = 0; i < ring.points.size(); ++i) {
                const QPointF lonLat = ring.points.at(i);
                const QPointF point = project(lonLat.x(), lonLat.y(), size);
                if (i == 0 || qAbs(lonLat.x() - ring.points.at(i - 1).x()) > 180.0)
                    path.moveTo(point);
                else
                    path.lineTo(point);
            }
            path.closeSubpath();
            painter->fillPath(path, active ? activeFill : idleFill);
            painter->strokePath(path, QPen(active ? activeStroke : idleStroke, 0.5));
        }
    }

    for (const CountryShape &shape : m_shapes) {
        const int count = counts.value(shape.code);
        if (count <= 0)
            continue;
        const QPointF point = project(shape.longitude, shape.latitude, size);
        const qreal radius = (3.0 + qMin(count, 12) * 0.4) * (size.width() / 900.0 + 0.75);
        QColor glow = m_accent;
        glow.setAlpha(51);
        painter->setPen(Qt::NoPen);
        painter->setBrush(glow);
        painter->drawEllipse(point, radius + 4, radius + 4);
        QColor ring = m_accent;
        ring.setAlpha(153);
        painter->setPen(QPen(ring, 1));
        painter->setBrush(Qt::NoBrush);
        painter->drawEllipse(point, radius + 2, radius + 2);
        painter->setPen(Qt::NoPen);
        painter->setBrush(m_accent);
        painter->drawEllipse(point, radius * 0.65, radius * 0.65);
        if (shape.code == m_selectedCode) {
            painter->setPen(QPen(QColor(236, 236, 239), 1.5));
            painter->setBrush(Qt::NoBrush);
            painter->drawEllipse(point, radius + 3, radius + 3);
        }
    }
}

void WorldMap::mousePressEvent(QMouseEvent *event) {
    ensureGeometry();
    const QHash<QString, int> counts = countByCode();
    const QSizeF size = boundingRect().size();
    QString nearest;
    qreal best = std::numeric_limits<qreal>::max();
    for (const CountryShape &shape : m_shapes) {
        if (counts.value(shape.code) <= 0)
            continue;
        const QPointF point = project(shape.longitude, shape.latitude, size);
        const qreal distance = QLineF(point, event->position()).length();
        if (distance < best) {
            best = distance;
            nearest = shape.code;
        }
    }
    const qreal threshold = 24.0 * (size.width() / 900.0 + 0.75);
    setSelectedCode(best <= threshold ? nearest : QString());
    event->accept();
}
