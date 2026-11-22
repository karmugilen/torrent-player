#include "connections.h"

#include "library.h"

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QtMath>

#include <algorithm>
#include <cmath>

namespace {

QString displayAddress(const QString &address) {
    // Android shows the peer address as the engine returns it (store screenshots
    // that blur IPs are edited by hand; the app itself does not blur).
    return address;
}

QString flagsFor(const PeerInfo &peer) {
    QStringList parts;
    if (!peer.transport.isEmpty())
        parts << peer.transport;
    if (peer.holePunched)
        parts << QObject::tr("punched");
    if (peer.bittorrentVersion == QLatin1String("v2"))
        parts << QStringLiteral("BT2");
    else if (peer.bittorrentVersion == QLatin1String("v1"))
        parts << QStringLiteral("BT1");
    return parts.join(QStringLiteral(" · "));
}

double optionalNumber(const QJsonValue &value) {
    if (value.isUndefined() || value.isNull())
        return -1;
    const double number = value.toDouble(-1);
    return std::isfinite(number) ? number : -1;
}

} // namespace

PeerListModel::PeerListModel(QObject *parent) : QAbstractListModel(parent) {}

int PeerListModel::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : m_peers.size();
}

QVariant PeerListModel::data(const QModelIndex &index, int role) const {
    if (!index.isValid() || index.row() < 0 || index.row() >= m_peers.size())
        return {};
    const PeerInfo &peer = m_peers.at(index.row());
    switch (role) {
    case AddressRole:
        return peer.address;
    case DisplayAddressRole:
        return displayAddress(peer.address);
    case TransportRole:
        return peer.transport;
    case ClientRole:
        return peer.client.isEmpty() ? QObject::tr("Unknown client") : peer.client;
    case CountryRole:
        return peer.country;
    case DownloadedRole:
        return double(peer.downloaded);
    case UploadedRole:
        return double(peer.uploaded);
    case DownloadRateRole:
        return peer.downloadRate;
    case ProgressRole:
        return peer.progress;
    case SourceRole:
        return peer.source;
    case BittorrentVersionRole:
        return peer.bittorrentVersion;
    case HolePunchedRole:
        return peer.holePunched;
    case FlagsRole:
        return flagsFor(peer);
    default:
        return {};
    }
}

QHash<int, QByteArray> PeerListModel::roleNames() const {
    return {{AddressRole, "address"},
            {DisplayAddressRole, "displayAddress"},
            {TransportRole, "transport"},
            {ClientRole, "client"},
            {CountryRole, "country"},
            {DownloadedRole, "downloaded"},
            {UploadedRole, "uploaded"},
            {DownloadRateRole, "downloadRate"},
            {ProgressRole, "progress"},
            {SourceRole, "source"},
            {BittorrentVersionRole, "bittorrentVersion"},
            {HolePunchedRole, "holePunched"},
            {FlagsRole, "flags"}};
}

void PeerListModel::setPeers(QVector<PeerInfo> peers) {
    beginResetModel();
    m_peers = std::move(peers);
    applySort();
    endResetModel();
}

void PeerListModel::sortBy(const QString &roleName, bool ascending) {
    if (m_sortRole == roleName && m_ascending == ascending)
        return;
    beginResetModel();
    m_sortRole = roleName;
    m_ascending = ascending;
    applySort();
    endResetModel();
}

void PeerListModel::applySort() {
    const QString role = m_sortRole;
    const bool ascending = m_ascending;
    std::sort(m_peers.begin(), m_peers.end(), [&](const PeerInfo &a, const PeerInfo &b) {
        auto cmp = [&](auto left, auto right) {
            if (left == right)
                return a.address < b.address;
            return ascending ? left < right : left > right;
        };
        if (role == QLatin1String("client"))
            return cmp(a.client.toLower(), b.client.toLower());
        if (role == QLatin1String("country"))
            return cmp(a.country.toLower(), b.country.toLower());
        if (role == QLatin1String("transport") || role == QLatin1String("flags"))
            return cmp(flagsFor(a).toLower(), flagsFor(b).toLower());
        if (role == QLatin1String("downloaded"))
            return cmp(a.downloaded, b.downloaded);
        if (role == QLatin1String("uploaded"))
            return cmp(a.uploaded, b.uploaded);
        if (role == QLatin1String("progress"))
            return cmp(a.progress, b.progress);
        if (role == QLatin1String("address") || role == QLatin1String("displayAddress"))
            return cmp(a.address, b.address);
        // Default: download rate (missing rates sort as zero).
        const double left = a.downloadRate > 0 ? a.downloadRate : 0;
        const double right = b.downloadRate > 0 ? b.downloadRate : 0;
        return cmp(left, right);
    });
}

Connections::Connections(QObject *parent) : QObject(parent), m_peerModel(this) {
    m_timer.setInterval(1000);
    connect(&m_timer, &QTimer::timeout, this, &Connections::poll);
}

void Connections::setEngine(Engine *engine) {
    if (m_engine == engine)
        return;
    m_engine = engine;
    emit engineChanged();
    updateTimer();
}

void Connections::setLibrary(Library *library) {
    if (m_library == library)
        return;
    m_library = library;
    emit libraryChanged();
    updateTimer();
    if (m_active)
        poll();
}

void Connections::setTorrentKey(const QString &key) {
    if (m_torrentKey == key)
        return;
    m_torrentKey = key;
    m_haveSnapshot = false;
    m_loading = !resolvedEngineId().isEmpty();
    m_unavailable = false;
    m_paused = false;
    m_peerModel.setPeers({});
    m_countries = {};
    emit torrentKeyChanged();
    emit snapshotChanged();
    updateTimer();
    if (m_active && !resolvedEngineId().isEmpty())
        poll();
}

void Connections::setEngineId(const QString &id) {
    if (m_engineId == id)
        return;
    m_engineId = id;
    m_haveSnapshot = false;
    m_loading = !resolvedEngineId().isEmpty();
    m_unavailable = false;
    m_paused = false;
    m_peerModel.setPeers({});
    m_countries = {};
    emit engineIdChanged();
    emit snapshotChanged();
    updateTimer();
    if (m_active && !resolvedEngineId().isEmpty())
        poll();
}

QString Connections::resolvedEngineId() const {
    if (!m_engineId.isEmpty())
        return m_engineId;
    if (m_library && !m_torrentKey.isEmpty()) {
        if (const Library::Entry *entry = m_library->entry(m_torrentKey)) {
            if (!entry->id.isEmpty())
                return entry->id;
        }
    }
    // Task fallback: treat the info-hash key as the engine id when nothing else is known.
    return m_torrentKey;
}

void Connections::setActive(bool active) {
    if (m_active == active)
        return;
    m_active = active;
    emit activeChanged();
    updateTimer();
    if (m_active && !m_engineId.isEmpty())
        poll();
    if (!m_active) {
        m_haveSnapshot = false;
        m_loading = false;
        m_unavailable = false;
        m_peerModel.setPeers({});
        m_countries = {};
        emit snapshotChanged();
    }
}

QString Connections::status() const {
    if (resolvedEngineId().isEmpty())
        return tr("No active torrent session");
    if (m_paused)
        return tr("Paused · peers disconnected");
    if (m_unavailable)
        return tr("Connection map unavailable");
    if (!m_haveSnapshot || m_loading)
        return tr("Finding connections…");
    if (peerCount() == 0)
        return tr("Waiting for peers");
    return tr("%n peers", "", peerCount());
}

void Connections::setSortRole(const QString &role) {
    if (role.isEmpty() || role == m_peerModel.sortRole())
        return;
    m_peerModel.sortBy(role, m_peerModel.sortAscending());
    emit sortChanged();
}

void Connections::setSortAscending(bool ascending) {
    if (ascending == m_peerModel.sortAscending())
        return;
    m_peerModel.sortBy(m_peerModel.sortRole(), ascending);
    emit sortChanged();
}

void Connections::toggleSort(const QString &role) {
    if (role == m_peerModel.sortRole())
        m_peerModel.sortBy(role, !m_peerModel.sortAscending());
    else
        m_peerModel.sortBy(role, role == QLatin1String("client") || role == QLatin1String("country")
                                     || role == QLatin1String("address") || role == QLatin1String("displayAddress")
                                     || role == QLatin1String("flags") || role == QLatin1String("transport"));
    emit sortChanged();
}

QVector<PeerInfo> Connections::parsePeers(const QJsonObject &root) {
    QVector<PeerInfo> peers;
    const QJsonArray array = root.value(QStringLiteral("peers")).toArray();
    peers.reserve(array.size());
    for (const QJsonValue &value : array) {
        const QJsonObject peer = value.toObject();
        PeerInfo row;
        row.address = peer.value(QStringLiteral("address")).toString();
        row.transport = peer.value(QStringLiteral("transport")).toString(QStringLiteral("Unknown"));
        row.client = peer.value(QStringLiteral("client")).toString();
        row.downloaded = qint64(peer.value(QStringLiteral("downloaded")).toDouble());
        row.uploaded = qint64(peer.value(QStringLiteral("uploaded")).toDouble());
        const QString country = peer.value(QStringLiteral("country")).toString();
        if (!country.isEmpty())
            row.country = country;
        row.source = peer.value(QStringLiteral("source")).toString();
        row.bittorrentVersion = peer.value(QStringLiteral("bittorrentVersion")).toString();
        row.holePunched = row.source == QLatin1String("C");
        row.downloadRate = optionalNumber(peer.value(QStringLiteral("downloadRate")));
        row.progress = optionalNumber(peer.value(QStringLiteral("peerCompletion")));
        if (row.address.isEmpty())
            continue;
        peers.push_back(row);
    }
    return peers;
}

bool Connections::parsePaused(const QJsonObject &root) {
    return root.value(QStringLiteral("paused")).toBool();
}

QVariantList Connections::countryCounts(const QVector<PeerInfo> &peers,
                                        const QHash<QString, QVariantMap> &labels) {
    QHash<QString, int> counts;
    for (const PeerInfo &peer : peers) {
        if (!peer.country.isEmpty())
            counts[peer.country.toUpper()] += 1;
    }
    QVariantList rows;
    for (auto it = counts.constBegin(); it != counts.constEnd(); ++it) {
        const QVariantMap label = labels.value(it.key());
        if (label.isEmpty())
            continue;
        QVariantMap row = label;
        row.insert(QStringLiteral("count"), it.value());
        rows.append(row);
    }
    std::sort(rows.begin(), rows.end(), [](const QVariant &left, const QVariant &right) {
        const QVariantMap a = left.toMap();
        const QVariantMap b = right.toMap();
        const int countA = a.value(QStringLiteral("count")).toInt();
        const int countB = b.value(QStringLiteral("count")).toInt();
        if (countA != countB)
            return countA > countB;
        return a.value(QStringLiteral("code")).toString() < b.value(QStringLiteral("code")).toString();
    });
    return rows;
}

QHash<QString, QVariantMap> Connections::loadMapLabels(const QByteArray &json) {
    QHash<QString, QVariantMap> labels;
    const QJsonArray array = QJsonDocument::fromJson(json).array();
    for (const QJsonValue &value : array) {
        const QJsonObject country = value.toObject();
        const QString code = country.value(QStringLiteral("code")).toString().toUpper();
        if (code.isEmpty())
            continue;
        labels.insert(code, {{"code", code},
                             {"name", country.value(QStringLiteral("name")).toString()},
                             {"longitude", country.value(QStringLiteral("longitude")).toDouble()},
                             {"latitude", country.value(QStringLiteral("latitude")).toDouble()}});
    }
    return labels;
}

void Connections::updateTimer() {
    if (m_active && m_engine && !resolvedEngineId().isEmpty())
        m_timer.start();
    else
        m_timer.stop();
}

void Connections::poll() {
    const QString id = resolvedEngineId();
    if (!m_engine || id.isEmpty() || !m_active)
        return;
    ensureLabels();
    const int generation = ++m_requestGeneration;
    const QString path = QStringLiteral("/connections/%1").arg(id);
    m_engine->get(path, [this, generation](const QJsonObject &body, int status) {
        if (generation != m_requestGeneration || !m_active)
            return;
        applySnapshot(body, status);
    });
}

void Connections::applySnapshot(const QJsonObject &body, int status) {
    m_loading = false;
    if (status != 200) {
        m_unavailable = true;
        m_haveSnapshot = false;
        m_paused = false;
        m_peerModel.setPeers({});
        m_countries = {};
        emit snapshotChanged();
        return;
    }
    m_unavailable = false;
    m_haveSnapshot = true;
    m_paused = parsePaused(body);
    QVector<PeerInfo> peers = m_paused ? QVector<PeerInfo>{} : parsePeers(body);
    m_countries = countryCounts(peers, m_labels);
    m_peerModel.setPeers(std::move(peers));
    emit snapshotChanged();
}

void Connections::ensureLabels() {
    if (!m_labels.isEmpty())
        return;
    QFile file(QStringLiteral(":/world-map.json"));
    if (!file.open(QIODevice::ReadOnly))
        return;
    m_labels = loadMapLabels(file.readAll());
}
