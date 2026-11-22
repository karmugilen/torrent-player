#pragma once

#include <QColor>
#include <QHash>
#include <QQuickPaintedItem>
#include <QVariantList>
#include <QVector>

// Equirectangular world map with country polygons from world-map.json and dots
// sized by peer count at each country's label point (same asset as Android).
class WorldMap : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(QVariantList countries READ countries WRITE setCountries NOTIFY countriesChanged)
    Q_PROPERTY(QColor accent READ accent WRITE setAccent NOTIFY colorsChanged)
    Q_PROPERTY(QColor land READ land WRITE setLand NOTIFY colorsChanged)
    Q_PROPERTY(QColor landActive READ landActive WRITE setLandActive NOTIFY colorsChanged)
    Q_PROPERTY(QColor grid READ grid WRITE setGrid NOTIFY colorsChanged)
    Q_PROPERTY(QColor background READ background WRITE setBackground NOTIFY colorsChanged)
    Q_PROPERTY(QString selectedCode READ selectedCode WRITE setSelectedCode NOTIFY selectedCodeChanged)
    Q_PROPERTY(QString summary READ summary NOTIFY summaryChanged)

public:
    explicit WorldMap(QQuickItem *parent = nullptr);

    QVariantList countries() const { return m_countries; }
    void setCountries(const QVariantList &countries);

    QColor accent() const { return m_accent; }
    void setAccent(const QColor &color);
    QColor land() const { return m_land; }
    void setLand(const QColor &color);
    QColor landActive() const { return m_landActive; }
    void setLandActive(const QColor &color);
    QColor grid() const { return m_grid; }
    void setGrid(const QColor &color);
    QColor background() const { return m_background; }
    void setBackground(const QColor &color);

    QString selectedCode() const { return m_selectedCode; }
    void setSelectedCode(const QString &code);
    QString summary() const { return m_summary; }

    void paint(QPainter *painter) override;

signals:
    void countriesChanged();
    void colorsChanged();
    void selectedCodeChanged();
    void summaryChanged();

protected:
    void mousePressEvent(QMouseEvent *event) override;
    void geometryChange(const QRectF &newGeometry, const QRectF &oldGeometry) override;

private:
    struct Ring {
        QVector<QPointF> points; // lon, lat
    };
    struct CountryShape {
        QString code;
        QString name;
        double longitude = 0;
        double latitude = 0;
        QVector<Ring> rings;
    };

    void ensureGeometry();
    void rebuildSummary();
    QPointF project(double longitude, double latitude, const QSizeF &size) const;
    QHash<QString, int> countByCode() const;

    QVariantList m_countries;
    QVector<CountryShape> m_shapes;
    bool m_geometryLoaded = false;
    bool m_geometryFailed = false;
    QColor m_accent{0xFF, 0xD6, 0x0A};
    QColor m_land{0x2A, 0x2A, 0x2F};
    QColor m_landActive{0xFF, 0xD6, 0x0A, 46};
    QColor m_grid{0x3A, 0x3A, 0x42};
    QColor m_background{0x11, 0x11, 0x14};
    QString m_selectedCode;
    QString m_summary;
};
