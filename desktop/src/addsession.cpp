#include "addsession.h"

#include "engine.h"
#include "library.h"
#include "links.h"

#include <QDateTime>
#include <QFile>
#include <QNetworkReply>
#include <QStorageInfo>
#include <QUrl>

static constexpr qint64 MaxTorrentFileBytes = 16 * 1024 * 1024;

AddSession::AddSession(Engine *engine, Library *library, QObject *parent)
    : QObject(parent), m_engine(engine), m_library(library) {
    m_poll.setInterval(700);
    connect(&m_poll, &QTimer::timeout, this, &AddSession::pollFiles);
    // A link opened while the engine is still starting waits for it.
    connect(m_engine, &Engine::started, this, [this] {
        if (m_state == "starting" && !m_pendingBody.isEmpty()) {
            const QJsonObject body = m_pendingBody;
            m_pendingBody = {};
            addToEngine(body);
        }
    });
}

void AddSession::reset() {
    ++m_generation;
    m_poll.stop();
    m_state = "idle";
    m_error.clear();
    m_name.clear();
    m_id.clear();
    m_key.clear();
    m_magnet.clear();
    m_existingKey.clear();
    m_torrentData.clear();
    m_pendingBody = {};
    m_files = {};
    m_selected.clear();
    m_peers = 0;
    m_waited = 0;
}

void AddSession::fail(const QString &message) {
    m_poll.stop();
    m_state = "error";
    m_error = message;
    emit changed();
}

void AddSession::start(const QString &input) {
    cancel();
    const QString text = input.trimmed();
    switch (Links::classify(text)) {
    case Links::Kind::Invalid:
        fail(text.startsWith("magnet:", Qt::CaseInsensitive)
                 ? tr("This magnet link has no info hash.")
                 : tr("Paste a magnet link or a link to a .torrent file, or open a .torrent file."));
        return;
    case Links::Kind::Magnet: {
        const QString hash = Links::magnetInfoHash(text);
        if (m_library->contains(hash)) {
            m_existingKey = hash;
            fail(tr("This torrent is already in your library."));
            return;
        }
        m_magnet = text;
        addToEngine({{"torrentId", text}, {"prepare", true}});
        return;
    }
    case Links::Kind::TorrentFile: {
        QFile file(Links::localPath(text));
        if (file.size() > MaxTorrentFileBytes || !file.open(QIODevice::ReadOnly)) {
            fail(tr("Could not open %1").arg(file.fileName()));
            return;
        }
        m_torrentData = file.readAll();
        addToEngine({{"torrentData", QString::fromLatin1(m_torrentData.toBase64())}, {"prepare", true}});
        return;
    }
    case Links::Kind::TorrentUrl: {
        m_state = "loading";
        emit changed();
        QNetworkRequest request{QUrl(text)};
        request.setAttribute(QNetworkRequest::RedirectPolicyAttribute, QNetworkRequest::NoLessSafeRedirectPolicy);
        request.setTransferTimeout(30000);
        QNetworkReply *reply = m_network.get(request);
        const int generation = m_generation;
        connect(reply, &QNetworkReply::downloadProgress, reply, [reply](qint64 received, qint64) {
            if (received > MaxTorrentFileBytes)
                reply->abort();
        });
        connect(reply, &QNetworkReply::finished, this, [this, reply, generation] {
            reply->deleteLater();
            if (generation != m_generation)
                return;
            const QByteArray data = reply->readAll();
            if (reply->error() != QNetworkReply::NoError) {
                fail(tr("Could not download the torrent file: %1").arg(reply->errorString()));
                return;
            }
            // Bencoded torrents start with a dictionary.
            if (!data.startsWith('d')) {
                fail(tr("That link is not a .torrent file."));
                return;
            }
            m_torrentData = data;
            addToEngine({{"torrentData", QString::fromLatin1(data.toBase64())}, {"prepare", true}});
        });
        return;
    }
    }
}

void AddSession::addToEngine(const QJsonObject &body) {
    if (!m_engine->ready()) {
        if (!m_engine->error().isEmpty()) {
            fail(m_engine->error());
            return;
        }
        m_pendingBody = body;
        m_state = "starting";
        emit changed();
        return;
    }
    m_state = "fetching";
    m_startedAt = QDateTime::currentMSecsSinceEpoch();
    emit changed();
    const int generation = m_generation;
    m_engine->post("/add", body, [this, generation](const QJsonObject &reply, int status) {
        if (generation != m_generation)
            return;
        if (status != 200 || reply.value("id").toString().isEmpty()) {
            fail(tr("The engine refused this torrent: %1").arg(reply.value("error").toString()));
            return;
        }
        const QString hash = reply.value("infoHash").toString().toLower();
        if (!hash.isEmpty() && m_library->contains(hash)) {
            // The engine hands back the torrent the library already runs; leave it be.
            m_existingKey = hash;
            fail(tr("This torrent is already in your library."));
            return;
        }
        m_id = reply.value("id").toString();
        m_key = hash;
        pollFiles();
        m_poll.start();
    });
}

void AddSession::pollFiles() {
    if (m_id.isEmpty())
        return;
    m_waited = int((QDateTime::currentMSecsSinceEpoch() - m_startedAt) / 1000);
    const int generation = m_generation;
    m_engine->get("/torrent/" + m_id, [this, generation](const QJsonObject &status, int code) {
        if (generation != m_generation || m_state != "fetching")
            return;
        if (code != 200) {
            fail(status.value("error").toString(tr("The torrent went away.")));
            return;
        }
        m_peers = status.value("numPeers").toInt();
        m_name = status.value("name").toString();
        if (m_key.isEmpty())
            m_key = status.value("infoHash").toString().toLower();
        if (m_magnet.isEmpty())
            m_magnet = status.value("magnetURI").toString();
        const QJsonArray files = status.value("files").toArray();
        if (status.value("ready").toBool() && !files.isEmpty()) {
            m_poll.stop();
            m_files = {};
            for (const auto &v : files) {
                const QJsonObject f = v.toObject();
                m_files.append(QJsonObject{{"index", f.value("index")}, {"name", f.value("name")},
                                           {"path", f.value("path")}, {"length", f.value("length")},
                                           {"type", f.value("type")}});
            }
            m_selected = defaultSelection(m_files);
            m_state = "choose";
        }
        emit changed();
    });
}

QList<int> AddSession::defaultSelection(const QJsonArray &files) {
    QStringList videos;
    for (const auto &v : files) {
        const QJsonObject f = v.toObject();
        if (Links::isVideo(f.value("name").toString(), f.value("type").toString()))
            videos << f.value("name").toString();
    }
    QList<int> selected;
    for (const auto &v : files) {
        const QJsonObject f = v.toObject();
        const QString name = f.value("name").toString();
        bool pick = videos.isEmpty() || Links::isVideo(name, f.value("type").toString());
        for (const QString &video : std::as_const(videos))
            pick = pick || Links::subtitleMatches(video, name);
        // A single video's subtitles are its own even when named differently.
        pick = pick || (videos.size() == 1 && Links::isSubtitle(name));
        if (pick)
            selected << f.value("index").toInt();
    }
    return selected;
}

QVariantList AddSession::files() const {
    QVariantList list;
    for (const auto &v : m_files) {
        const QJsonObject f = v.toObject();
        const int index = f.value("index").toInt();
        const QString name = f.value("name").toString();
        list.append(QVariantMap{{"index", index},
                                {"name", name},
                                {"path", f.value("path").toString()},
                                {"length", f.value("length").toDouble()},
                                {"video", Links::isVideo(name, f.value("type").toString())},
                                {"subtitle", Links::isSubtitle(name)},
                                {"selected", m_selected.contains(index)}});
    }
    return list;
}

double AddSession::selectedBytes() const {
    double total = 0;
    for (const auto &v : m_files)
        if (m_selected.contains(v.toObject().value("index").toInt()))
            total += v.toObject().value("length").toDouble();
    return total;
}

double AddSession::freeBytes() const {
    return double(QStorageInfo(m_engine->dataDirectory()).bytesAvailable());
}

bool AddSession::selectedHasVideo() const {
    for (const auto &v : m_files) {
        const QJsonObject f = v.toObject();
        if (m_selected.contains(f.value("index").toInt()) &&
            Links::isVideo(f.value("name").toString(), f.value("type").toString()))
            return true;
    }
    return false;
}

void AddSession::toggle(int index) {
    if (m_selected.contains(index))
        m_selected.removeAll(index);
    else
        m_selected.append(index);
    std::sort(m_selected.begin(), m_selected.end());
    emit changed();
}

void AddSession::selectAll() {
    m_selected.clear();
    for (const auto &v : m_files)
        m_selected << v.toObject().value("index").toInt();
    emit changed();
}

void AddSession::selectNone() {
    m_selected.clear();
    emit changed();
}

void AddSession::selectVideos() {
    m_selected.clear();
    for (const auto &v : m_files) {
        const QJsonObject f = v.toObject();
        if (Links::isVideo(f.value("name").toString(), f.value("type").toString()))
            m_selected << f.value("index").toInt();
    }
    emit changed();
}

void AddSession::confirm(bool playNow) {
    if (m_state != "choose" || m_selected.isEmpty())
        return;
    const QString id = m_id, key = m_key, magnet = m_magnet, name = m_name;
    const QJsonArray files = m_files;
    const QList<int> selected = m_selected;
    // From here the torrent belongs to the library: closing the dialog must not remove it.
    m_id.clear();
    m_state = "saving";
    emit changed();
    auto finish = [this, id, key, magnet, name, files, selected, playNow](const QByteArray &torrentData) {
        m_library->adopt(id, key, magnet, name, files, selected, torrentData);
        reset();
        emit changed();
        emit added(key, playNow);
    };
    if (!m_torrentData.isEmpty()) {
        finish(m_torrentData);
        return;
    }
    // Keep the metadata, so the next start doesn't wait for peers to send it.
    m_engine->get("/metadata/" + id, [finish](const QJsonObject &reply, int status) {
        finish(status == 200 ? QByteArray::fromBase64(reply.value("torrentData").toString().toLatin1()) : QByteArray());
    });
}

void AddSession::cancel() {
    if (!m_id.isEmpty())
        m_engine->post("/remove", {{"id", m_id}, {"destroyStore", true}});
    reset();
    emit changed();
}
