#pragma once

#include "engine.h"
#include "library.h"

#include <QAbstractListModel>
#include <QHash>
#include <QJsonObject>
#include <QObject>
#include <QTimer>
#include <QVariantList>
#include <QVector>

// One connected peer from GET /connections/<id>, as the Android app reads it.
struct PeerInfo {
    QString address;
    QString transport;
    QString client;
    QString country;
    qint64 downloaded = 0;
    qint64 uploaded = 0;
    // Negative means the engine omitted the field.
    double downloadRate = -1;
    double progress = -1;
    QString source;
    QString bittorrentVersion;
    bool holePunched = false;
};

// Sortable list of peers for the Peers page.
class PeerListModel : public QAbstractListModel {
    Q_OBJECT

public:
    enum Role {
        AddressRole = Qt::UserRole + 1,
        DisplayAddressRole,
        TransportRole,
        ClientRole,
        CountryRole,
        DownloadedRole,
        UploadedRole,
        DownloadRateRole,
        ProgressRole,
        SourceRole,
        BittorrentVersionRole,
        HolePunchedRole,
        FlagsRole,
    };

    explicit PeerListModel(QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    void setPeers(QVector<PeerInfo> peers);
    void sortBy(const QString &roleName, bool ascending);
    QString sortRole() const { return m_sortRole; }
    bool sortAscending() const { return m_ascending; }

private:
    void applySort();

    QVector<PeerInfo> m_peers;
    QString m_sortRole = QStringLiteral("downloadRate");
    bool m_ascending = false;
};

// Polls /connections/<engineId> while visible and exposes peers plus per-country
// counts with map label points (from world-map.json).
class Connections : public QObject {
    Q_OBJECT
    Q_PROPERTY(Engine *engine READ engine WRITE setEngine NOTIFY engineChanged)
    Q_PROPERTY(Library *library READ library WRITE setLibrary NOTIFY libraryChanged)
    Q_PROPERTY(QString torrentKey READ torrentKey WRITE setTorrentKey NOTIFY torrentKeyChanged)
    Q_PROPERTY(QString engineId READ engineId WRITE setEngineId NOTIFY engineIdChanged)
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY activeChanged)
    Q_PROPERTY(bool paused READ paused NOTIFY snapshotChanged)
    Q_PROPERTY(bool loading READ loading NOTIFY snapshotChanged)
    Q_PROPERTY(bool unavailable READ unavailable NOTIFY snapshotChanged)
    Q_PROPERTY(QString status READ status NOTIFY snapshotChanged)
    Q_PROPERTY(int peerCount READ peerCount NOTIFY snapshotChanged)
    Q_PROPERTY(QObject *peers READ peers CONSTANT)
    Q_PROPERTY(QVariantList countries READ countries NOTIFY snapshotChanged)
    Q_PROPERTY(QString sortRole READ sortRole WRITE setSortRole NOTIFY sortChanged)
    Q_PROPERTY(bool sortAscending READ sortAscending WRITE setSortAscending NOTIFY sortChanged)

public:
    explicit Connections(QObject *parent = nullptr);

    Engine *engine() const { return m_engine; }
    void setEngine(Engine *engine);

    Library *library() const { return m_library; }
    void setLibrary(Library *library);

    QString torrentKey() const { return m_torrentKey; }
    void setTorrentKey(const QString &key);

    // Explicit override; when empty, resolved from library.entry(torrentKey)->id,
    // then torrentKey (details() does not currently expose engineId).
    QString engineId() const { return m_engineId; }
    void setEngineId(const QString &id);

    bool active() const { return m_active; }
    void setActive(bool active);

    bool paused() const { return m_paused; }
    bool loading() const { return m_loading; }
    bool unavailable() const { return m_unavailable; }
    QString status() const;
    int peerCount() const { return m_peerModel.rowCount(); }
    QObject *peers() { return &m_peerModel; }
    QVariantList countries() const { return m_countries; }

    QString sortRole() const { return m_peerModel.sortRole(); }
    void setSortRole(const QString &role);
    bool sortAscending() const { return m_peerModel.sortAscending(); }
    void setSortAscending(bool ascending);

    Q_INVOKABLE void toggleSort(const QString &role);

    // Parses the engine JSON the Android client reads. Exposed for unit tests.
    static QVector<PeerInfo> parsePeers(const QJsonObject &root);
    static bool parsePaused(const QJsonObject &root);
    // Builds country rows with label points from world-map.json metadata.
    static QVariantList countryCounts(const QVector<PeerInfo> &peers,
                                      const QHash<QString, QVariantMap> &labels);
    static QHash<QString, QVariantMap> loadMapLabels(const QByteArray &json);

signals:
    void engineChanged();
    void libraryChanged();
    void torrentKeyChanged();
    void engineIdChanged();
    void activeChanged();
    void snapshotChanged();
    void sortChanged();

private:
    QString resolvedEngineId() const;
    void updateTimer();
    void poll();
    void applySnapshot(const QJsonObject &body, int status);
    void ensureLabels();

    Engine *m_engine = nullptr;
    Library *m_library = nullptr;
    QString m_torrentKey;
    QString m_engineId;
    bool m_active = false;
    bool m_paused = false;
    bool m_loading = false;
    bool m_unavailable = false;
    bool m_haveSnapshot = false;
    PeerListModel m_peerModel;
    QVariantList m_countries;
    QHash<QString, QVariantMap> m_labels;
    QTimer m_timer;
    int m_requestGeneration = 0;
};
