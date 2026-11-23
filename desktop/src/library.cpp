#include "library.h"

#include "engine.h"
#include "links.h"
#include "policy.h"
#include "settings.h"

#include <QClipboard>
#include <QDateTime>
#include <QDesktopServices>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QGuiApplication>
#include <QImage>
#include <QJsonDocument>
#include <QPointer>
#include <QProcess>
#include <QSaveFile>
#include <QStandardPaths>
#include <QTimer>
#include <QUrl>
#include <QVideoFrame>
#include <QVideoSink>
#include <QtConcurrent>

#include "platform.h"

#ifdef Q_OS_WIN
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <unistd.h>
#endif

// A hard link: no space and no time. False when the OS or file system says no.
static bool hardLink(const QString &source, const QString &target) {
#ifdef Q_OS_WIN
    return CreateHardLinkW(reinterpret_cast<LPCWSTR>(QDir::toNativeSeparators(target).utf16()),
                           reinterpret_cast<LPCWSTR>(QDir::toNativeSeparators(source).utf16()), nullptr) != 0;
#else
    return ::link(QFile::encodeName(source).constData(), QFile::encodeName(target).constData()) == 0;
#endif
}

Library::Library(Engine *engine, const QString &dataDirectory, const QString &saveDirectory, QObject *parent)
    : QAbstractListModel(parent), m_engine(engine), m_dataDirectory(dataDirectory), m_saveDirectory(saveDirectory) {
    QDir().mkpath(m_dataDirectory + "/torrents");
    QDir().mkpath(m_dataDirectory + "/thumbnails");
    load();
    m_poll.setInterval(1000);
    connect(&m_poll, &QTimer::timeout, this, &Library::poll);
    connect(m_engine, &Engine::started, this, &Library::restoreAll);
    connect(m_engine, &Engine::stateChanged, this, [this] {
        if (m_engine->ready())
            return;
        // The engine went away: its torrent ids mean nothing any more.
        for (Entry &entry : m_entries) {
            entry.id.clear();
            entry.configured = false;
            entry.attaching = false;
            entry.downloadSpeed = entry.uploadSpeed = 0;
            entry.peers = 0;
        }
        touch({});
    });
}

void Library::setSettings(Settings *settings) {
    m_settings = settings;
    m_activeTransfers = -1;
    touch({});
}

QJsonObject Library::playPolicy() const {
    return m_settings ? m_settings->policy().play() : Policy::instance().play();
}

int Library::idleDropMinutes() const {
    return m_settings ? m_settings->idleDropMinutes() : Policy::instance().defaultValue("idleDropMinutes").toInt();
}

qint64 Library::streamCacheBytes() const {
    // Android directPlayMemoryBytes: the RAM cache size in use, sent for both modes.
    return m_settings ? m_settings->ramCacheBytes()
                      : qint64(Policy::instance().defaultValue("ramCacheMiB").toInt()) * 1024 * 1024;
}

// ---------------------------------------------------------------- model

int Library::rowCount(const QModelIndex &parent) const {
    return parent.isValid() ? 0 : m_entries.size();
}

QHash<int, QByteArray> Library::roleNames() const {
    return {{KeyRole, "key"},           {NameRole, "name"},       {ProgressRole, "progress"},
            {DownloadSpeedRole, "downloadSpeed"}, {UploadSpeedRole, "uploadSpeed"}, {PeersRole, "peers"},
            {SizeRole, "size"},         {DownloadedRole, "downloaded"}, {StateRole, "state"},
            {EtaRole, "eta"},           {ThumbnailRole, "thumbnail"}, {ErrorRole, "error"},
            {HasVideoRole, "hasVideo"}, {AddedRole, "addedAt"},
            {DirectPlayRole, "directPlay"}};
}

static qint64 selectedLength(const Library::Entry &entry) {
    qint64 total = 0;
    for (const auto &value : entry.files) {
        const QJsonObject file = value.toObject();
        if (entry.selected.contains(file.value("index").toInt()))
            total += qint64(file.value("length").toDouble());
    }
    return total;
}

static bool hasVideo(const Library::Entry &entry) {
    for (const auto &value : entry.files) {
        const QJsonObject file = value.toObject();
        if (Links::isVideo(file.value("name").toString(), file.value("type").toString()))
            return true;
    }
    return false;
}

QVariant Library::data(const QModelIndex &index, int role) const {
    if (!index.isValid() || index.row() >= m_entries.size())
        return {};
    const Entry &e = m_entries.at(index.row());
    switch (role) {
    case KeyRole: return e.key;
    case NameRole: return e.name.isEmpty() ? e.key : e.name;
    case ProgressRole: return e.progress;
    case DownloadSpeedRole: return e.downloadSpeed;
    case UploadSpeedRole: return e.uploadSpeed;
    case PeersRole: return e.peers;
    case SizeRole: return double(selectedLength(e));
    case DownloadedRole: return double(e.downloaded);
    case StateRole: return displayState(e);
    case EtaRole: return e.etaMs / 1000.0;
    case ThumbnailRole: return e.thumbnail.isEmpty() ? QString() : QUrl::fromLocalFile(e.thumbnail).toString();
    case ErrorRole: return e.error;
    case HasVideoRole: return hasVideo(e);
    case AddedRole: return double(e.addedAt);
    case DirectPlayRole: return e.directPlay;
    }
    return {};
}

QString Library::displayState(const Entry &e) {
    if (!e.error.isEmpty())
        return "error";
    if (e.paused)
        return "paused";
    if (e.id.isEmpty())
        return e.progress >= 1 ? "done" : "starting";
    if (!e.ready)
        return "fetching";
    if (e.checking)
        return "checking";
    if (e.done || e.progress >= 1)
        return "done";
    if (e.downloadSpeed > 0)
        return "downloading";
    if (e.peers == 0)
        return "searching";
    return "waiting";
}

double Library::downloadSpeed() const {
    double total = 0;
    for (const Entry &e : m_entries)
        total += e.downloadSpeed;
    return total;
}

double Library::uploadSpeed() const {
    double total = 0;
    for (const Entry &e : m_entries)
        total += e.uploadSpeed;
    return total;
}

int Library::activeCount() const {
    int active = 0;
    for (const Entry &e : m_entries)
        if (!e.paused && e.progress < 1 && e.error.isEmpty())
            ++active;
    return active;
}

int Library::row(const QString &key) const {
    for (int i = 0; i < m_entries.size(); ++i)
        if (m_entries[i].key == key)
            return i;
    return -1;
}

Library::Entry *Library::find(const QString &key) {
    const int i = row(key);
    return i < 0 ? nullptr : &m_entries[i];
}

const Library::Entry *Library::entry(const QString &key) const {
    const int i = row(key);
    return i < 0 ? nullptr : &m_entries[i];
}

bool Library::contains(const QString &key) const {
    return row(key) >= 0;
}

void Library::touch(const QString &key) {
    if (key.isEmpty()) {
        if (!m_entries.isEmpty())
            emit dataChanged(index(0), index(m_entries.size() - 1));
    } else if (const int i = row(key); i >= 0) {
        emit dataChanged(index(i), index(i));
    }
    ++m_revision;
    emit revisionChanged();
    // The Auto RAM cache shares memory among running torrents (Android applyAutoRamCache).
    const int active = activeTransfers(m_entries);
    if (m_settings && active != m_activeTransfers) {
        m_activeTransfers = active;
        m_settings->setActiveTransfers(active);
    }
}

// ---------------------------------------------------------------- disk

QString Library::torrentFilePath(const QString &key) const {
    return m_dataDirectory + "/torrents/" + key + ".torrent";
}

QString Library::thumbnailPath(const QString &key) const {
    return m_dataDirectory + "/thumbnails/" + key + ".jpg";
}

QString Library::partPath(const QString &key, int fileIndex) const {
    // engine-rs keeps files it opens itself (no descriptor passed in) here;
    // see wt-storage engine_owned_part_path.
    return m_engine->dataDirectory() + "/.boundary/" + key + "/" + QString::number(fileIndex) + ".part";
}

QString Library::exportPath(const Entry &entry, int fileIndex) const {
    for (const auto &value : entry.files) {
        const QJsonObject file = value.toObject();
        if (file.value("index").toInt() != fileIndex)
            continue;
        // Torrent paths come from strangers: keep them inside the save folder.
        QStringList parts;
        for (const QString &part : file.value("path").toString().split('/', Qt::SkipEmptyParts))
            if (part != "." && part != "..")
                parts << part;
        if (parts.isEmpty())
            parts << QString("file-%1").arg(fileIndex);
        if (parts.size() == 1 && !entry.name.isEmpty() && entry.files.size() > 1)
            parts.prepend(QString(entry.name).replace('/', '_'));
        return m_saveDirectory + "/" + parts.join('/');
    }
    return {};
}

QJsonObject Library::toJson(const Entry &e) {
    QJsonArray selected, exported;
    for (int i : e.selected)
        selected.append(i);
    QList<int> exportedSorted(e.exported.begin(), e.exported.end());
    std::sort(exportedSorted.begin(), exportedSorted.end());
    for (int i : exportedSorted)
        exported.append(i);
    return {{"key", e.key},         {"magnet", e.magnet},     {"name", e.name},
            {"selected", selected}, {"files", e.files},       {"paused", e.paused},
            {"addedAt", double(e.addedAt)}, {"exported", exported}, {"thumbnail", e.thumbnail},
            {"positions", e.positions}, {"durations", e.durations}, {"lastFile", e.lastFile},
            {"watchedAt", e.watchedAt}, {"progress", e.progress}, {"directPlay", e.directPlay}};
}

Library::Entry Library::fromJson(const QJsonObject &o) {
    Entry e;
    e.key = o.value("key").toString().toLower();
    e.magnet = o.value("magnet").toString();
    e.name = o.value("name").toString();
    for (const auto &v : o.value("selected").toArray())
        e.selected.append(v.toInt());
    e.files = o.value("files").toArray();
    e.paused = o.value("paused").toBool();
    e.addedAt = qint64(o.value("addedAt").toDouble());
    for (const auto &v : o.value("exported").toArray())
        e.exported.insert(v.toInt());
    e.thumbnail = o.value("thumbnail").toString();
    e.positions = o.value("positions").toObject();
    e.durations = o.value("durations").toObject();
    e.watchedAt = o.value("watchedAt").toObject();
    e.directPlay = o.value("directPlay").toBool();
    e.lastFile = o.value("lastFile").toInt(-1);
    e.progress = o.value("progress").toDouble();
    return e;
}

void Library::load() {
    QFile file(m_dataDirectory + "/library.json");
    if (!file.open(QIODevice::ReadOnly))
        return;
    const QJsonArray items = QJsonDocument::fromJson(file.readAll()).object().value("torrents").toArray();
    for (const auto &item : items) {
        Entry e = fromJson(item.toObject());
        if (e.key.isEmpty() || contains(e.key))
            continue;
        if (!e.thumbnail.isEmpty() && !QFileInfo::exists(e.thumbnail))
            e.thumbnail.clear();
        // Direct Play stays dropped after a restart until played (Android start()).
        if (e.directPlay)
            e.paused = true;
        m_entries.append(e);
    }
}

void Library::save() {
    QJsonArray items;
    for (const Entry &e : m_entries)
        items.append(toJson(e));
    // Written beside the target and renamed, so a crash never leaves half a library.
    QSaveFile file(m_dataDirectory + "/library.json");
    if (!file.open(QIODevice::WriteOnly)) {
        emit message(tr("Could not save the library: %1").arg(file.errorString()));
        return;
    }
    file.write(QJsonDocument(QJsonObject{{"version", 1}, {"torrents", items}}).toJson());
    if (!file.commit())
        emit message(tr("Could not save the library: %1").arg(file.errorString()));
}

// ---------------------------------------------------------------- engine

void Library::restoreAll() {
    m_engine->get("/stats", [this](const QJsonObject &stats, int) {
        m_streamPort = stats.value("streamPort").toInt();
    });
    for (Entry &entry : m_entries)
        if (!(entry.directPlay && entry.paused))
            attach(entry);
    m_poll.start();
}

void Library::attach(Entry &entry) {
    entry.id.clear();
    entry.configured = false;
    entry.error.clear();
    entry.attaching = true;
    QJsonObject body{{"torrentId", entry.magnet}, {"prepare", true}};
    // The saved .torrent means no waiting for peers to send metadata again.
    QFile torrent(torrentFilePath(entry.key));
    if (torrent.open(QIODevice::ReadOnly))
        body.insert("torrentData", QString::fromLatin1(torrent.readAll().toBase64()));
    const QString key = entry.key;
    m_engine->post("/add", body, [this, key](const QJsonObject &reply, int status) {
        Entry *e = find(key);
        if (!e)
            return;
        e->attaching = false;
        if (status != 200 || reply.value("id").toString().isEmpty()) {
            e->error = reply.value("error").toString(tr("The engine refused this torrent."));
            m_pendingPlay.remove(key);
            touch(key);
            return;
        }
        e->id = reply.value("id").toString();
        configure(key);
    });
}

static QJsonArray nullDescriptors(int count) {
    // A null descriptor tells engine-rs to open the file itself in its data
    // directory; Android passes file descriptors from its storage framework.
    QJsonArray descriptors;
    for (int i = 0; i < count; ++i)
        descriptors.append(QJsonValue::Null);
    return descriptors;
}

void Library::configure(const QString &key) {
    Entry *e = find(key);
    if (!e || e->id.isEmpty())
        return;
    QJsonArray selected;
    for (int i : e->selected)
        selected.append(i);
    if (m_settings)
        m_settings->refreshAutoRamCache();
    const QJsonObject body{{"id", e->id},
                           {"descriptors", nullDescriptors(e->files.size())},
                           {"selected", selected},
                           {"storageMode", e->directPlay ? "stream" : "download"},
                           {"streamCacheBytes", double(streamCacheBytes())}};
    m_engine->post("/configure", body, [this, key](const QJsonObject &reply, int status) {
        Entry *e = find(key);
        if (!e)
            return;
        // /add may hand back a record that is already set up (Android restoreEntry).
        if (status != 200 && !reply.value("error").toString().contains("already configured", Qt::CaseInsensitive)) {
            e->error = reply.value("error").toString(tr("Could not start the download."));
            m_pendingPlay.remove(key);
            touch(key);
            return;
        }
        e->configured = true;
        if (e->paused)
            m_engine->post("/pause", {{"id", e->id}});
        else
            m_engine->post("/resume", {{"id", e->id}});
        touch(key);
        if (m_pendingPlay.contains(key)) {
            const int file = m_pendingPlay.take(key);
            QTimer::singleShot(0, this, [this, key, file] { play(key, file); });
        }
    });
}

void Library::poll() {
    if (!m_engine->ready())
        return;
    if (m_streamPort == 0)
        m_engine->get("/stats", [this](const QJsonObject &stats, int) {
            m_streamPort = stats.value("streamPort").toInt();
        });
    for (const Entry &entry : std::as_const(m_entries)) {
        if (entry.id.isEmpty())
            continue;
        const QString key = entry.key;
        const QString id = entry.id;
        m_engine->get("/torrent/" + entry.id, [this, key, id](const QJsonObject &status, int code) {
            Entry *e = find(key);
            // Dropped or re-added meanwhile: this answer is about an old engine record.
            if (!e || e->id != id)
                return;
            if (code == 200)
                update(*e, status);
            else if (code == 404) {
                // The engine dropped it (it restarted or removed it): add it again.
                attach(*e);
            }
        });
    }
}

void Library::update(Entry &e, const QJsonObject &s) {
    const bool wasDone = e.progress >= 1;
    e.ready = s.value("ready").toBool();
    e.done = s.value("done").toBool();
    e.checking = s.value("checking").toBool();
    e.progress = s.value("progress").toDouble();
    e.downloadSpeed = s.value("downloadSpeed").toDouble();
    e.uploadSpeed = s.value("uploadSpeed").toDouble();
    e.peers = s.value("numPeers").toInt();
    e.downloaded = qint64(s.value("verifiedBytes").toDouble());
    e.etaMs = s.value("timeRemaining").toDouble();
    e.seeding = s.value("seeding").toBool();
    e.idleDropReady = s.value("idleDropReady").toBool();
    e.playbackReaders = s.value("playbackReaders").toInt();
    e.nextPreload = s.value("nextPreload").toObject();
    const QString error = s.value("error").toString();
    if (!error.isEmpty())
        e.error = error;
    if (e.name.isEmpty() || e.name == e.key)
        e.name = s.value("name").toString(e.name);
    const QJsonArray files = s.value("files").toArray();
    if (!files.isEmpty()) {
        e.liveFiles = files;
        if (e.files.isEmpty()) {
            for (const auto &v : files) {
                const QJsonObject f = v.toObject();
                e.files.append(QJsonObject{{"index", f.value("index")}, {"name", f.value("name")},
                                           {"path", f.value("path")}, {"length", f.value("length")},
                                           {"type", f.value("type")}});
            }
        }
    }
    exportFinished(e);
    makeThumbnail(e);
    if (!wasDone && e.progress >= 1)
        save();
    if (shouldDropIdle(e, idleDropMinutes(), m_playingKey == e.key)) {
        dropEntry(e, true);
        return;
    }
    touch(e.key);
}

// ---------------------------------------------------------------- finished files

void Library::exportFinished(Entry &e) {
    // Direct Play streams only: nothing is saved to Downloads.
    if (e.directPlay)
        return;
    for (const auto &value : std::as_const(e.liveFiles)) {
        const QJsonObject file = value.toObject();
        const int index = file.value("index").toInt();
        const qint64 length = qint64(file.value("length").toDouble());
        if (!e.selected.contains(index) || e.exported.contains(index) || e.exporting.contains(index))
            continue;
        if (qint64(file.value("verifiedBytes").toDouble()) < length || length <= 0)
            continue;
        const QString source = partPath(e.key, index);
        const QString target = exportPath(e, index);
        if (target.isEmpty() || !QFileInfo::exists(source))
            continue;
        e.exporting.insert(index);
        const QString key = e.key;
        QPointer<Library> self(this);
        (void)QtConcurrent::run([self, key, index, source, target, length] {
            QString error;
            QString destination = target;
            const QFileInfo existing(destination);
            // Never overwrite something already in Downloads that isn't ours.
            if (existing.exists() && existing.size() != length) {
                const QString base = existing.path() + "/" + existing.completeBaseName();
                const QString suffix = existing.suffix().isEmpty() ? QString() : "." + existing.suffix();
                for (int n = 1; QFileInfo::exists(destination); ++n)
                    destination = QString("%1 (%2)%3").arg(base).arg(n).arg(suffix);
            }
            if (!QFileInfo::exists(destination)) {
                QDir().mkpath(QFileInfo(destination).path());
                const QString temporary = destination + ".torrentplayer-tmp";
                QFile::remove(temporary);
                // A hard link costs no space and no time; across file systems, copy.
                bool ok = hardLink(source, temporary);
                if (!ok)
                    ok = QFile::copy(source, temporary);
                if (ok)
                    ok = QFile::rename(temporary, destination);
                if (!ok) {
                    QFile::remove(temporary);
                    error = QObject::tr("Could not save %1 to %2").arg(QFileInfo(target).fileName(), QFileInfo(target).path());
                }
            }
            QMetaObject::invokeMethod(qApp, [self, key, index, error] {
                if (!self)
                    return;
                Entry *e = self->find(key);
                if (!e)
                    return;
                e->exporting.remove(index);
                if (error.isEmpty()) {
                    e->exported.insert(index);
                    self->save();
                } else {
                    emit self->message(error);
                }
                self->touch(key);
            });
        });
    }
}

// ---------------------------------------------------------------- thumbnails

void Library::makeThumbnail(Entry &e) {
    if (!e.thumbnail.isEmpty() || e.thumbnailBusy)
        return;
    // A frame the player saved (thumbnailPath).
    if (QFileInfo::exists(thumbnailPath(e.key))) {
        e.thumbnail = thumbnailPath(e.key);
        save();
        return;
    }
    if (e.thumbnailAttempts >= 4 || e.id.isEmpty())
        return;
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (now - e.thumbnailTriedAt < 30000)
        return;

    // A poster that came with the torrent is the best picture.
    for (const auto &value : std::as_const(e.liveFiles)) {
        const QJsonObject file = value.toObject();
        const QString name = file.value("name").toString();
        const int index = file.value("index").toInt();
        const qint64 length = qint64(file.value("length").toDouble());
        if (!Links::isImage(name) || length <= 0 || length > 20 * 1024 * 1024)
            continue;
        if (qint64(file.value("verifiedBytes").toDouble()) < length)
            continue;
        QImage image(partPath(e.key, index));
        if (image.isNull())
            continue;
        const QString path = thumbnailPath(e.key);
        if (image.scaledToWidth(qMin(640, image.width()), Qt::SmoothTransformation).save(path, "JPG", 85)) {
            e.thumbnail = path;
            save();
            return;
        }
    }

    // Otherwise a frame from the main video, from disk when it's complete,
    // else through the engine's stream once some of it has arrived.
    const int video = defaultVideo(e.key);
    if (video < 0)
        return;
    QJsonObject file;
    for (const auto &value : std::as_const(e.liveFiles))
        if (value.toObject().value("index").toInt() == video)
            file = value.toObject();
    const qint64 length = qint64(file.value("length").toDouble());
    const qint64 verified = qint64(file.value("verifiedBytes").toDouble());
    const bool complete = length > 0 && verified >= length;
    if (!complete && (verified < 16 * 1024 * 1024 || m_playingKey == e.key || m_streamPort == 0))
        return;
    if (!e.selected.contains(video))
        return;

    e.thumbnailBusy = true;
    e.thumbnailTriedAt = now;
    ++e.thumbnailAttempts;
    const QString key = e.key;
    const QString out = thumbnailPath(key) + ".tmp.jpg";
    // The first minute is often black titles, so look further in first; a
    // short clip has nothing there, so later attempts look near the start.
    const QString seek = e.thumbnailAttempts <= 2 ? "60" : "3";
    auto run = [this, key, out, seek](const QString &input) {
        auto *process = new QProcess(this);
        process->start("ffmpeg", {"-v", "error", "-y", "-ss", seek, "-i", input, "-frames:v", "1",
                                  "-vf", "scale=640:-2", "-q:v", "4", out});
        QTimer::singleShot(90000, process, [process] { process->kill(); });
        connect(process, &QProcess::finished, this, [this, process, key, out] {
            process->deleteLater();
            finishThumbnail(key, out);
        });
        connect(process, &QProcess::errorOccurred, this, [this, process, key](QProcess::ProcessError error) {
            if (error != QProcess::FailedToStart)
                return;
            process->deleteLater();
            if (Entry *e = find(key)) {
                e->thumbnailBusy = false;
                e->thumbnailAttempts = 99; // ffmpeg is missing; don't keep trying.
            }
        });
    };
    if (complete) {
        run(partPath(key, video));
        return;
    }
    m_engine->post("/play", {{"id", e.id}, {"fileIndex", video}}, [this, key, run](const QJsonObject &reply, int status) {
        const QString url = reply.value("streamUrl").toString();
        if (status != 200 || url.isEmpty()) {
            if (Entry *e = find(key))
                e->thumbnailBusy = false;
            return;
        }
        run(url);
    });
}

void Library::finishThumbnail(const QString &key, const QString &path) {
    Entry *e = find(key);
    QImage image(path);
    if (!e) {
        QFile::remove(path);
        return;
    }
    e->thumbnailBusy = false;
    if (image.isNull() || image.width() < 16) {
        QFile::remove(path);
        return;
    }
    const QString final = thumbnailPath(key);
    QFile::remove(final);
    if (QFile::rename(path, final)) {
        e->thumbnail = final;
        save();
        touch(key);
    }
}

void Library::captureThumbnail(const QString &key, QObject *videoSink) {
    Entry *e = find(key);
    auto *sink = qobject_cast<QVideoSink *>(videoSink);
    if (!e || !sink || !e->thumbnail.isEmpty())
        return;
    const QImage image = sink->videoFrame().toImage();
    if (image.isNull())
        return;
    const QString path = thumbnailPath(key);
    if (image.scaledToWidth(qMin(640, image.width()), Qt::SmoothTransformation).save(path, "JPG", 85)) {
        e->thumbnail = path;
        save();
        touch(key);
    }
}

// ---------------------------------------------------------------- actions

QString Library::adopt(const QString &id, const QString &key, const QString &magnet, const QString &name,
                       const QJsonArray &files, const QList<int> &selected, const QByteArray &torrentData,
                       bool directPlay) {
    if (Entry *existing = find(key))
        return existing->key;
    if (!torrentData.isEmpty()) {
        QSaveFile file(torrentFilePath(key));
        if (file.open(QIODevice::WriteOnly)) {
            file.write(torrentData);
            file.commit();
        }
    }
    Entry e;
    e.key = key;
    e.id = id;
    e.magnet = magnet;
    e.name = name;
    e.files = files;
    e.selected = selected;
    e.directPlay = directPlay;
    e.addedAt = QDateTime::currentMSecsSinceEpoch();
    beginInsertRows({}, 0, 0);
    m_entries.prepend(e);
    endInsertRows();
    emit countChanged();
    save();
    configure(key);
    if (!m_poll.isActive())
        m_poll.start();
    touch(key);
    return key;
}

static QJsonObject fileByIndex(const QJsonArray &files, int index) {
    for (const auto &value : files)
        if (value.toObject().value("index").toInt() == index)
            return value.toObject();
    return {};
}

static QVariantList playlistItems(const Library::Entry &e, int fileIndex);

QString Library::finishedFilePath(const QString &key, int fileIndex) const {
    const Entry *e = entry(key);
    if (!e)
        return {};
    const QJsonObject file = fileByIndex(e->liveFiles, fileIndex);
    const qint64 length = qint64(file.value("length").toDouble());
    if (length <= 0 || qint64(file.value("verifiedBytes").toDouble()) < length)
        return {};
    if (e->exported.contains(fileIndex) && QFileInfo::exists(exportPath(*e, fileIndex)))
        return exportPath(*e, fileIndex);
    return QFileInfo::exists(partPath(key, fileIndex)) ? partPath(key, fileIndex) : QString();
}

int Library::defaultVideo(const QString &key) const {
    const Entry *e = entry(key);
    return e ? Series::resumeIndex(seriesFiles(*e)) : -1;
}

QVariantList Library::subtitlesFor(const Entry &e, int videoIndex) const {
    const QString videoName = fileByIndex(e.files, videoIndex).value("name").toString();
    QVariantList matching, others;
    for (const auto &value : e.liveFiles.isEmpty() ? e.files : e.liveFiles) {
        const QJsonObject file = value.toObject();
        const QString name = file.value("name").toString();
        const int index = file.value("index").toInt();
        if (!Links::isSubtitle(name))
            continue;
        const qint64 length = qint64(file.value("length").toDouble());
        const bool ready = e.selected.contains(index) && qint64(file.value("verifiedBytes").toDouble()) >= length;
        QVariantMap subtitle{{"label", name}, {"index", index}, {"ready", ready},
                             {"path", ready ? partPath(e.key, index) : QString()}};
        (Links::subtitleMatches(videoName, name) ? matching : others).append(subtitle);
    }
    return matching + others;
}

void Library::play(const QString &key, int fileIndex) {
    Entry *e = find(key);
    if (!e)
        return;
    if (fileIndex < 0)
        fileIndex = defaultVideo(key);
    if (fileIndex < 0) {
        emit message(tr("There is no video in this torrent."));
        return;
    }
    if (!m_engine->ready()) {
        emit message(tr("The torrent engine is not running."));
        return;
    }
    if (e->id.isEmpty() || !e->configured) {
        // Dropped (Direct Play) or still starting: play once it is set up again.
        m_pendingPlay.insert(key, fileIndex);
        if (e->paused) {
            e->paused = false;
            save();
        }
        if (e->id.isEmpty() && !e->attaching)
            attach(*e);
        touch(key);
        return;
    }
    if (!e->selected.contains(fileIndex)) {
        // Playing a file that wasn't picked adds it to the torrent.
        setFileSelected(key, fileIndex, true);
        QTimer::singleShot(300, this, [this, key, fileIndex] { startPlay(key, fileIndex); });
    } else {
        startPlay(key, fileIndex);
    }
}

void Library::startPlay(const QString &key, int fileIndex) {
    Entry *e = find(key);
    if (!e || e->id.isEmpty())
        return;
    // Android play(): a previous file-only playback ends before the next starts.
    if (!m_fileOnlyId.isEmpty()) {
        m_engine->post("/file-only", {{"id", m_fileOnlyId}, {"enabled", false}});
        m_fileOnlyId.clear();
    }
    if (e->paused) {
        e->paused = false;
        m_engine->post("/resume", {{"id", e->id}});
        save();
    }
    const QJsonObject policy = playPolicy();
    const QJsonObject body = playBody(*e, fileIndex, policy);
    const bool preload = policy.value("preloadStart").toBool();
    const QString id = e->id;
    m_engine->post("/play", body, [this, key, id, fileIndex, body, preload](const QJsonObject &reply, int status) {
        Entry *e = find(key);
        if (!e || e->id != id)
            return;
        const QString url = reply.value("streamUrl").toString();
        if (status != 200 || url.isEmpty()) {
            emit message(tr("Could not play this file: %1").arg(reply.value("error").toString()));
            return;
        }
        if (body.value("fileOnly").toBool())
            m_fileOnlyId = id;
        // The engine puts the playing file first.
        m_engine->post("/focus", {{"id", id}, {"fileIndex", fileIndex}});
        e->lastFile = fileIndex;
        m_playingKey = key;
        m_playingFile = fileIndex;
        m_playingId = id;
        save();
        const QJsonObject file = fileByIndex(e->files, fileIndex);
        const qint64 length = qint64(file.value("length").toDouble());
        const QString name = file.value("name").toString(reply.value("name").toString());
        auto open = [this, key, id, fileIndex, url, name, length] {
            const Entry *e = entry(key);
            if (!e || e->id != id)
                return;
            const QString index = QString::number(fileIndex);
            emit playReady({{"key", key},
                            {"engineId", id},
                            {"fileIndex", fileIndex},
                            {"url", url},
                            {"title", name},
                            {"length", double(length)},
                            {"fileLength", double(length)},
                            {"durationMs", e->durations.value(index).toDouble()},
                            {"directPlay", e->directPlay},
                            {"startMs", e->positions.value(index).toDouble()},
                            {"nextFileIndex", nextVideoAfter(*e, fileIndex)},
                            {"episode", Series::episodeTitle(name)},
                            {"playlist", playlistItems(*e, fileIndex)},
                            {"playNext", playPolicy().value("playNext").toBool()},
                            {"playNextCountdownSeconds", playPolicy().value("playNextCountdownSeconds").toInt(5)},
                            {"subtitles", subtitlesFor(*e, fileIndex)}});
        };
        if (preload)
            awaitPreload(key, id, fileIndex, length, QDateTime::currentMSecsSinceEpoch() + 8000, open);
        else
            open();
    });
}

// Android awaitPlaybackPreload: up to 8 s for the first 2 MiB of the file.
void Library::awaitPreload(const QString &key, const QString &id, int fileIndex, qint64 length, qint64 deadline,
                           std::function<void()> done) {
    m_engine->get("/torrent/" + id, [this, key, id, fileIndex, length, deadline, done](const QJsonObject &status, int code) {
        const Entry *e = entry(key);
        if (!e || e->id != id)
            return;
        const qint64 needed = qMin<qint64>(2 * 1024 * 1024, qMax<qint64>(0, length));
        const QJsonObject file = fileByIndex(status.value("files").toArray(), fileIndex);
        const bool ready = code == 200 && !file.isEmpty() &&
                           (qint64(file.value("verifiedBytes").toDouble()) >= needed || file.value("progress").toDouble() >= 1.0);
        const qint64 remaining = deadline - QDateTime::currentMSecsSinceEpoch();
        if (ready || remaining <= 0 || code == 404) {
            done();
            return;
        }
        QTimer::singleShot(qMin<qint64>(400, remaining), this, [this, key, id, fileIndex, length, deadline, done] {
            awaitPreload(key, id, fileIndex, length, deadline, done);
        });
    });
}

void Library::endPlayback() {
    // Android: the player is gone, so no file is focused and file-only playback ends.
    const auto alive = [this](const QString &id) {
        for (const Entry &e : std::as_const(m_entries))
            if (e.id == id)
                return true;
        return false;
    };
    if (!m_playingId.isEmpty() && alive(m_playingId))
        m_engine->post("/focus", {{"id", m_playingId}, {"fileIndex", QJsonValue::Null}});
    if (!m_fileOnlyId.isEmpty() && alive(m_fileOnlyId))
        m_engine->post("/file-only", {{"id", m_fileOnlyId}, {"enabled", false}});
    m_fileOnlyId.clear();
    m_playingKey.clear();
    m_playingFile = -1;
    m_playingId.clear();
}

void Library::savePosition(const QString &key, int fileIndex, qint64 positionMs, qint64 durationMs) {
    Entry *e = find(key);
    if (!e)
        return;
    const QString index = QString::number(fileIndex);
    if (durationMs < 0)
        durationMs = qint64(e->durations.value(index).toDouble());
    if (durationMs > 0)
        e->durations.insert(index, double(durationMs));
    e->positions.insert(index, double(savedPosition(positionMs, durationMs)));
    // Android bookmarkUpdatedAt: only a real place counts as watched.
    if (positionMs > 0)
        e->watchedAt.insert(index, double(QDateTime::currentMSecsSinceEpoch()));
    // A new file may already be playing (the next episode): only its own close ends playback.
    if (m_playingKey == key && m_playingFile == fileIndex)
        endPlayback();
    save();
}

void Library::setPlaybackDuration(const QString &key, int fileIndex, qint64 durationMs) {
    Entry *e = find(key);
    if (!e || durationMs <= 0)
        return;
    const QString index = QString::number(fileIndex);
    if (qint64(e->durations.value(index).toDouble()) == durationMs)
        return;
    e->durations.insert(index, double(durationMs));
    save();
}

// ---------------------------------------------------------------- play rules

qint64 Library::savedPosition(qint64 positionMs, qint64 durationMs) {
    if (durationMs > 0 && positionMs >= durationMs - 3000)
        return 0;
    const qint64 position = qMax<qint64>(0, positionMs);
    return durationMs > 0 ? qMin(position, durationMs) : position;
}

qint64 Library::resumeByteHint(qint64 length, qint64 positionMs, qint64 durationMs) {
    if (length <= 0 || positionMs < 0 || durationMs <= 0)
        return -1;
    const double fraction = qBound(0.0, double(positionMs) / double(durationMs), 1.0);
    return qBound<qint64>(0, qint64(double(length) * fraction), length - 1);
}

static bool verifiedComplete(const Library::Entry &e, int index) {
    const QJsonObject file = fileByIndex(e.liveFiles, index);
    const qint64 length = qint64(file.value("length").toDouble());
    return length > 0 && qint64(file.value("verifiedBytes").toDouble()) >= length;
}

static bool isVideoFile(const QJsonObject &file) {
    return Links::isVideo(file.value("name").toString(), file.value("type").toString()) ||
           Links::isVideo(file.value("path").toString());
}

int Library::nextVideoAfter(const Entry &e, int endedIndex) {
    const QJsonObject ended = fileByIndex(e.files, endedIndex);
    if (ended.isEmpty())
        return -1;
    using Key = std::pair<QString, int>;
    const Key endedKey{Links::naturalMediaKey(ended.value("path").toString()), endedIndex};
    int best = -1;
    Key bestKey;
    for (const auto &value : e.files) {
        const QJsonObject file = value.toObject();
        const int index = file.value("index").toInt();
        const QString path = file.value("path").toString();
        if (index == endedIndex || !isVideoFile(file) || Links::isExtraVideo(path))
            continue;
        if (!e.selected.contains(index) && !verifiedComplete(e, index))
            continue;
        const Key key{Links::naturalMediaKey(path), index};
        if (!(endedKey < key))
            continue;
        if (best < 0 || key < bestKey) {
            best = index;
            bestKey = key;
        }
    }
    if (best < 0)
        return -1;
    // Already watched to the end: nothing to play next.
    const QString index = QString::number(best);
    const qint64 duration = qint64(e.durations.value(index).toDouble());
    const qint64 position = qint64(e.positions.value(index).toDouble());
    if (duration > 0 && position >= duration - 3000)
        return -1;
    return best;
}

int Library::nextEpisodeToWarm(const Entry &e, int playingIndex, bool playNext) {
    if (!playNext || !isVideoFile(fileByIndex(e.files, playingIndex)))
        return -1;
    const int next = nextVideoAfter(e, playingIndex);
    return next >= 0 && e.selected.contains(next) && !verifiedComplete(e, next) ? next : -1;
}

QList<int> Library::playlist(const Entry &e, int fileIndex) {
    const QJsonObject current = fileByIndex(e.files, fileIndex);
    // Samples and trailers stay out of the episode run; one picked on its own plays alone.
    if (current.isEmpty() || Links::isExtraVideo(current.value("path").toString()))
        return {fileIndex};
    using Key = std::pair<QString, int>;
    QList<Key> videos;
    for (const auto &value : e.files) {
        const QJsonObject file = value.toObject();
        const int index = file.value("index").toInt();
        const QString path = file.value("path").toString();
        if (!isVideoFile(file) || Links::isExtraVideo(path))
            continue;
        if (!e.selected.contains(index) && (e.directPlay || !verifiedComplete(e, index)) && index != fileIndex)
            continue;
        videos.append({Links::naturalMediaKey(path), index});
    }
    std::sort(videos.begin(), videos.end());
    QList<int> list;
    for (const Key &key : videos)
        list.append(key.second);
    return list;
}

static QVariantList playlistItems(const Library::Entry &e, int fileIndex) {
    QVariantList items;
    for (int index : Library::playlist(e, fileIndex)) {
        const QString name = fileByIndex(e.files, index).value("name").toString();
        items.append(QVariantMap{{"index", index}, {"name", name}, {"title", Series::episodeTitle(name)}});
    }
    return items;
}

QList<Series::File> Library::seriesFiles(const Entry &e) {
    QList<Series::File> list;
    // Saved before watch times were kept: the last played file counts as the last watched.
    const bool legacy = e.watchedAt.isEmpty() && e.lastFile >= 0;
    for (const auto &value : e.files) {
        const QJsonObject file = value.toObject();
        Series::File f;
        f.index = file.value("index").toInt();
        const QString key = QString::number(f.index);
        f.name = file.value("name").toString();
        f.path = file.value("path").toString(f.name);
        f.length = qint64(file.value("length").toDouble());
        f.video = isVideoFile(file);
        f.selected = e.selected.contains(f.index);
        f.verifiedComplete = verifiedComplete(e, f.index);
        f.positionMs = qint64(e.positions.value(key).toDouble());
        f.durationMs = qint64(e.durations.value(key).toDouble());
        f.watchedAt = qint64(e.watchedAt.value(key).toDouble());
        if (legacy && f.index == e.lastFile && (f.positionMs > 0 || f.durationMs > 0))
            f.watchedAt = 1;
        list.append(f);
    }
    return list;
}

bool Library::shouldDropIdle(const Entry &e, int idleMinutes, bool playing) {
    return idleMinutes > 0 && e.directPlay && e.idleDropReady && e.playbackReaders == 0 && !e.id.isEmpty() &&
           !e.paused && !playing;
}

int Library::activeTransfers(const QList<Entry> &entries) {
    int active = 0;
    for (const Entry &e : entries)
        if (!e.paused && e.error.isEmpty() && (e.progress < 1 || e.seeding))
            ++active;
    return qMax(1, active);
}

QJsonObject Library::playBody(const Entry &e, int fileIndex, const QJsonObject &policy) {
    QJsonObject body{{"id", e.id}, {"fileIndex", fileIndex}};
    const QString index = QString::number(fileIndex);
    const qint64 hint = resumeByteHint(qint64(fileByIndex(e.files, fileIndex).value("length").toDouble()),
                                       qint64(e.positions.value(index).toDouble()),
                                       qint64(e.durations.value(index).toDouble()));
    if (hint >= 0)
        body.insert("offset", double(hint));
    // Flags go only when on, as EngineClient.play sends them.
    if (policy.value("boost").toBool())
        body.insert("boost", true);
    if (policy.value("fileOnly").toBool() && !e.directPlay)
        body.insert("fileOnly", true);
    if (policy.value("adaptiveBuffer").toBool())
        body.insert("adaptiveBuffer", true);
    if (policy.value("burstAwareBuffer").toBool())
        body.insert("burstAwareBuffer", true);
    const int next = nextEpisodeToWarm(e, fileIndex, policy.value("playNext").toBool());
    if (next >= 0)
        body.insert("nextFileIndex", next);
    return body;
}

// ---------------------------------------------------------------- transfer control

void Library::dropEntry(Entry &e, bool idle) {
    if (!e.directPlay)
        return;
    const QString id = e.id;
    if (idle) {
        if (id.isEmpty() || m_idleDropped.contains(id))
            return;
        m_idleDropped.insert(id);
    }
    if (!id.isEmpty())
        m_engine->post("/remove", {{"id", id}, {"destroyStore", false}});
    if (m_fileOnlyId == id)
        m_fileOnlyId.clear();
    if (m_playingId == id) {
        m_playingKey.clear();
        m_playingFile = -1;
        m_playingId.clear();
    }
    m_pendingPlay.remove(e.key);
    e.id.clear();
    e.configured = false;
    e.paused = true;
    e.downloadSpeed = e.uploadSpeed = 0;
    e.peers = 0;
    e.playbackReaders = 0;
    e.idleDropReady = false;
    const QString key = e.key, name = e.name.isEmpty() ? e.key : e.name;
    save();
    touch(key);
    if (idle)
        emit message(tr("Stopped idle Direct Play: %1").arg(name));
}

void Library::drop(const QString &key) {
    if (Entry *e = find(key))
        dropEntry(*e, false);
}

void Library::downloadInstead(const QString &key) {
    Entry *e = find(key);
    if (!e || !e->directPlay)
        return;
    e->directPlay = false;
    e->paused = false;
    m_pendingPlay.remove(key);
    save();
    const QString id = e->id;
    if (!id.isEmpty()) {
        // The engine record streams to RAM: add it again with files on disk.
        e->id.clear();
        e->configured = false;
        e->attaching = true;
        if (m_fileOnlyId == id)
            m_fileOnlyId.clear();
        m_engine->post("/remove", {{"id", id}, {"destroyStore", false}}, [this, key](const QJsonObject &, int) {
            if (Entry *e = find(key))
                attach(*e);
        });
    } else if (m_engine->ready() && !e->attaching) {
        attach(*e);
    }
    emit message(tr("Downloading to %1").arg(m_saveDirectory));
    touch(key);
}

void Library::pause(const QString &key) {
    Entry *e = find(key);
    if (!e || e->paused)
        return;
    // Direct Play has no paused state on Android: it is dropped (RAM freed).
    if (e->directPlay) {
        dropEntry(*e, false);
        return;
    }
    e->paused = true;
    if (!e->id.isEmpty())
        m_engine->post("/pause", {{"id", e->id}});
    e->downloadSpeed = e->uploadSpeed = 0;
    save();
    touch(key);
}

void Library::resume(const QString &key) {
    Entry *e = find(key);
    if (!e || !e->paused)
        return;
    e->paused = false;
    if (!e->id.isEmpty())
        m_engine->post("/resume", {{"id", e->id}});
    else if (m_engine->ready() && !e->attaching)
        attach(*e); // dropped Direct Play
    save();
    touch(key);
}

void Library::togglePause(const QString &key) {
    const Entry *e = entry(key);
    if (!e)
        return;
    e->paused ? resume(key) : pause(key);
}

void Library::retry(const QString &key) {
    Entry *e = find(key);
    if (!e)
        return;
    e->error.clear();
    if (e->id.isEmpty()) {
        attach(*e);
    } else {
        // Drop the engine's copy (keeping its files) and add it fresh.
        const QString id = e->id;
        m_engine->post("/remove", {{"id", id}, {"destroyStore", false}}, [this, key](const QJsonObject &, int) {
            if (Entry *e = find(key))
                attach(*e);
        });
    }
    touch(key);
}

void Library::remove(const QString &key, bool deleteFiles) {
    const int i = row(key);
    if (i < 0)
        return;
    const Entry e = m_entries.at(i);
    if (!e.id.isEmpty())
        m_engine->post("/remove", {{"id", e.id}, {"destroyStore", deleteFiles}});
    if (deleteFiles) {
        // Only the files this app saved, never anything else in the folder.
        for (int index : e.exported)
            QFile::remove(exportPath(e, index));
        const QString root = e.files.size() > 1 ? exportPath(e, e.files.first().toObject().value("index").toInt()) : QString();
        if (!root.isEmpty()) {
            // Remove folders left empty, up to the save folder.
            QDir dir = QFileInfo(root).dir();
            while (dir.absolutePath().startsWith(m_saveDirectory + "/") && dir.isEmpty() && dir.rmdir("."))
                dir.cdUp();
        }
        QDir(m_engine->dataDirectory() + "/.boundary/" + key).removeRecursively();
    }
    QFile::remove(torrentFilePath(key));
    QFile::remove(thumbnailPath(key));
    beginRemoveRows({}, i, i);
    m_entries.removeAt(i);
    endRemoveRows();
    emit countChanged();
    save();
    touch({});
}

void Library::setFileSelected(const QString &key, int fileIndex, bool selected) {
    Entry *e = find(key);
    if (!e || e->selected.contains(fileIndex) == selected)
        return;
    if (selected)
        e->selected.append(fileIndex);
    else
        e->selected.removeAll(fileIndex);
    std::sort(e->selected.begin(), e->selected.end());
    save();
    if (!e->id.isEmpty() && e->configured) {
        QJsonArray list;
        for (int i : e->selected)
            list.append(i);
        m_engine->post("/select", {{"id", e->id}, {"selected", list}, {"descriptors", nullDescriptors(e->files.size())}},
                       [this, key](const QJsonObject &reply, int status) {
                           if (status != 200)
                               emit message(tr("Could not change the files: %1").arg(reply.value("error").toString()));
                           touch(key);
                       });
    }
    touch(key);
}

QVariantMap Library::details(const QString &key) const {
    const Entry *e = entry(key);
    if (!e)
        return {};
    QVariantList files;
    const QJsonArray &source = e->liveFiles.isEmpty() ? e->files : e->liveFiles;
    // Natural order grouped by season, extras last (Android lists episodes in natural order).
    const QList<Series::File> series = seriesFiles(*e);
    QHash<int, Series::File> byIndex;
    for (const Series::File &f : series)
        byIndex.insert(f.index, f);
    QHash<int, QJsonObject> live;
    for (const auto &value : source)
        live.insert(value.toObject().value("index").toInt(), value.toObject());
    QList<int> order = Series::displayOrder(series);
    for (auto it = live.cbegin(); it != live.cend(); ++it)
        if (!byIndex.contains(it.key()))
            order.append(it.key());
    bool grouped = false;
    for (const Series::File &f : series)
        grouped = grouped || (f.video && !Series::groupOf(f).isEmpty());
    const bool paused = e->paused;
    for (int index : std::as_const(order)) {
        const QJsonObject f = live.value(index, fileByIndex(e->files, index));
        const QString name = f.value("name").toString();
        const qint64 length = qint64(f.value("length").toDouble());
        const qint64 verified = qint64(f.value("verifiedBytes").toDouble());
        files.append(QVariantMap{
            {"index", index},
            {"name", name},
            {"path", f.value("path").toString()},
            {"length", double(length)},
            {"progress", length > 0 ? double(verified) / double(length) : f.value("progress").toDouble()},
            {"selected", e->selected.contains(index)},
            {"video", Links::isVideo(name, f.value("type").toString())},
            {"subtitle", Links::isSubtitle(name)},
            {"saved", e->exported.contains(index)},
            {"savedPath", e->exported.contains(index) ? exportPath(*e, index) : QString()},
            {"positionMs", e->positions.value(QString::number(index)).toDouble()},
            {"durationMs", e->durations.value(QString::number(index)).toDouble()},
            {"episode", Series::episodeTitle(name)},
            {"group", grouped ? Series::groupOf(byIndex.value(index)) : QString()},
            {"upNext", paused ? QString() : Series::upNextLabel(e->nextPreload, index)},
        });
    }
    return {{"key", e->key},
            {"name", e->name.isEmpty() ? e->key : e->name},
            {"magnet", e->magnet},
            {"state", displayState(*e)},
            {"error", e->error},
            {"progress", e->progress},
            {"downloadSpeed", e->downloadSpeed},
            {"uploadSpeed", e->uploadSpeed},
            {"peers", e->peers},
            {"size", double(selectedLength(*e))},
            {"downloaded", double(e->downloaded)},
            {"eta", e->etaMs / 1000.0},
            {"paused", e->paused},
            {"directPlay", e->directPlay},
            {"thumbnail", e->thumbnail.isEmpty() ? QString() : QUrl::fromLocalFile(e->thumbnail).toString()},
            {"files", files},
            {"defaultVideo", defaultVideo(key)},
            {"grouped", grouped},
            {"engineId", e->id},
            {"addedAt", QDateTime::fromMSecsSinceEpoch(e->addedAt)}};
}

void Library::copyMagnet(const QString &key) const {
    const Entry *e = entry(key);
    if (!e)
        return;
    QGuiApplication::clipboard()->setText(e->magnet.isEmpty() ? "magnet:?xt=urn:btih:" + e->key : e->magnet);
}

void Library::copyText(const QString &text) const {
    QGuiApplication::clipboard()->setText(text);
}

bool Library::openInMpv(const QString &url, qint64 startMs, const QString &title) const {
    QStringList arguments{"--force-window=immediate", "--title=" + title};
    if (startMs > 0)
        arguments << QString("--start=%1").arg(startMs / 1000.0, 0, 'f', 1);
    arguments << url;
    const Platform::Os os = Platform::current();
    for (const QString &program : Platform::externalMpvCandidates(os, QStandardPaths::findExecutable("mpv")))
        if (os == Platform::Os::Linux || QFileInfo(program).isExecutable() || !program.contains('/'))
            if (QProcess::startDetached(program, arguments))
                return true;
    return false;
}

QString Library::clipboardText() const {
    return QGuiApplication::clipboard()->text().trimmed();
}

void Library::openFolder(const QString &key) const {
    const Entry *e = entry(key);
    QString folder = m_saveDirectory;
    if (e && !e->files.isEmpty()) {
        const QString first = exportPath(*e, e->files.first().toObject().value("index").toInt());
        if (QFileInfo::exists(QFileInfo(first).path()))
            folder = QFileInfo(first).path();
    }
    QDir().mkpath(folder);
    QDesktopServices::openUrl(QUrl::fromLocalFile(folder));
}
