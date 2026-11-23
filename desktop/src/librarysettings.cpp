#include "library.h"

#include "engine.h"

#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QFileInfo>
#include <QSet>
#include <QStorageInfo>

// Which .boundary/<hash> folders are safe to delete: on disk but not in the library.
QStringList orphanBoundaryKeys(const QStringList &onDisk, const QStringList &inLibrary) {
    const QSet<QString> keep(inLibrary.begin(), inLibrary.end());
    QStringList orphan;
    for (const QString &key : onDisk) {
        if (!key.isEmpty() && !keep.contains(key))
            orphan << key;
    }
    orphan.sort();
    return orphan;
}

void Library::setSaveDirectory(const QString &path) {
    const QString cleaned = QDir::cleanPath(path);
    if (cleaned.isEmpty() || cleaned == m_saveDirectory)
        return;
    QDir().mkpath(cleaned);
    m_saveDirectory = cleaned;
    emit saveDirectoryChanged();
    emit message(tr("New downloads will be saved to %1").arg(m_saveDirectory));
}

void Library::pauseAll() {
    for (const QString &key : libraryKeys())
        pause(key);
}

void Library::resumeAll() {
    for (const QString &key : libraryKeys())
        resume(key);
}

void Library::deleteDownloadedFiles() {
    for (Entry &e : m_entries) {
        for (int index : e.exported)
            QFile::remove(exportPath(e, index));
        const QString root =
            e.files.size() > 1 ? exportPath(e, e.files.first().toObject().value("index").toInt()) : QString();
        if (!root.isEmpty()) {
            QDir dir = QFileInfo(root).dir();
            while (dir.absolutePath().startsWith(m_saveDirectory + "/") && dir.isEmpty() && dir.rmdir("."))
                dir.cdUp();
        }
        e.exported.clear();
    }
    save();
    touch({});
    emit message(tr("Downloaded files were removed."));
}

QStringList Library::libraryKeys() const {
    QStringList keys;
    keys.reserve(m_entries.size());
    for (const Entry &e : m_entries)
        keys << e.key;
    return keys;
}

double Library::managedBytes() const {
    qint64 total = 0;
    for (const Entry &e : m_entries) {
        for (int index : e.exported) {
            const QFileInfo info(exportPath(e, index));
            if (info.exists())
                total += info.size();
        }
    }
    return double(total);
}

double Library::freeBytes() const {
    QStorageInfo storage(m_saveDirectory);
    if (!storage.isValid())
        storage = QStorageInfo(QFileInfo(m_saveDirectory).absolutePath());
    return storage.isValid() ? double(storage.bytesAvailable()) : -1.0;
}

qint64 Library::clearOrphanBoundaryCache() {
    const QString root = m_engine->dataDirectory() + "/.boundary";
    QDir dir(root);
    if (!dir.exists())
        return 0;
    QStringList onDisk = dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot);
    qint64 reclaimed = 0;
    for (const QString &key : orphanBoundaryKeys(onDisk, libraryKeys())) {
        const QString path = root + "/" + key;
        QDirIterator it(path, QDir::Files, QDirIterator::Subdirectories);
        qint64 bytes = 0;
        while (it.hasNext()) {
            it.next();
            bytes += it.fileInfo().size();
        }
        if (QDir(path).removeRecursively())
            reclaimed += bytes;
    }
    return reclaimed;
}
