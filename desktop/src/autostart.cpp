#include "autostart.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QSaveFile>
#include <QStandardPaths>
#include <QTextStream>

Autostart::Autostart(const QString &directory, QObject *parent) : QObject(parent) {
    m_directory = directory.isEmpty()
                      ? QStandardPaths::writableLocation(QStandardPaths::ConfigLocation) + "/autostart"
                      : directory;
}

QString Autostart::desktopFilePath() const { return m_directory + "/torrentplayer.desktop"; }

bool Autostart::enabled() const { return QFile::exists(desktopFilePath()); }

void Autostart::setEnabled(bool enabled) {
    if (enabled == this->enabled())
        return;
    if (enabled) {
        QDir().mkpath(m_directory);
        writeDesktopFile(desktopFilePath(), desktopFileContents());
    } else {
        removeDesktopFile(desktopFilePath());
    }
    emit enabledChanged();
}

QString Autostart::desktopFileContents(const QString &execLine) {
    return QStringLiteral(
               "[Desktop Entry]\n"
               "Type=Application\n"
               "Name=Torrent Player\n"
               "Comment=Stream and download torrents\n"
               "Exec=%1\n"
               "Icon=torrentplayer\n"
               "Terminal=false\n"
               "X-GNOME-Autostart-enabled=true\n"
               "Hidden=false\n")
        .arg(execLine);
}

bool Autostart::writeDesktopFile(const QString &path, const QString &contents) {
    QDir().mkpath(QFileInfo(path).absolutePath());
    QSaveFile file(path);
    if (!file.open(QIODevice::WriteOnly | QIODevice::Text))
        return false;
    QTextStream out(&file);
    out << contents;
    return file.commit();
}

bool Autostart::removeDesktopFile(const QString &path) { return !QFile::exists(path) || QFile::remove(path); }
