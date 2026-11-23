#include "autostart.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QSaveFile>
#include <QSettings>
#include <QStandardPaths>
#include <QTextStream>

#ifdef Q_OS_WIN
static const char *runKey = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
static const char *runValue = "TorrentPlayer";
#endif

Autostart::Autostart(const QString &directory, QObject *parent) : QObject(parent) {
    m_customDirectory = !directory.isEmpty();
    if (m_customDirectory)
        m_directory = directory;
#ifdef Q_OS_MACOS
    else
        m_directory = QDir::homePath() + "/Library/LaunchAgents";
#else
    else
        m_directory = QStandardPaths::writableLocation(QStandardPaths::ConfigLocation) + "/autostart";
#endif
}

bool Autostart::useRegistry() const {
#ifdef Q_OS_WIN
    return !m_customDirectory;
#else
    return false;
#endif
}

QString Autostart::desktopFilePath() const {
#ifdef Q_OS_MACOS
    return m_directory + "/" + launchAgentLabel + ".plist";
#else
    return m_directory + "/torrentplayer.desktop";
#endif
}

bool Autostart::enabled() const {
#ifdef Q_OS_WIN
    if (useRegistry())
        return QSettings(runKey, QSettings::NativeFormat).contains(runValue);
#endif
    return QFile::exists(desktopFilePath());
}

void Autostart::setEnabled(bool enabled) {
    if (enabled == this->enabled())
        return;
#ifdef Q_OS_WIN
    if (useRegistry()) {
        QSettings run(runKey, QSettings::NativeFormat);
        if (enabled)
            run.setValue(runValue, windowsRunCommand(QCoreApplication::applicationFilePath()));
        else
            run.remove(runValue);
        run.sync();
        emit enabledChanged();
        return;
    }
#endif
    if (enabled) {
        QDir().mkpath(m_directory);
#ifdef Q_OS_MACOS
        writeDesktopFile(desktopFilePath(), launchAgentContents(QCoreApplication::applicationFilePath()));
#else
        writeDesktopFile(desktopFilePath(), desktopFileContents());
#endif
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

static QString xmlEscaped(QString text) {
    return text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
}

QString Autostart::launchAgentContents(const QString &program) {
    return QStringLiteral(
               "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
               "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" "
               "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n"
               "<plist version=\"1.0\">\n"
               "<dict>\n"
               "    <key>Label</key>\n"
               "    <string>%1</string>\n"
               "    <key>ProgramArguments</key>\n"
               "    <array>\n"
               "        <string>%2</string>\n"
               "        <string>--background</string>\n"
               "    </array>\n"
               "    <key>RunAtLoad</key>\n"
               "    <true/>\n"
               "    <key>ProcessType</key>\n"
               "    <string>Interactive</string>\n"
               "</dict>\n"
               "</plist>\n")
        .arg(QLatin1String(launchAgentLabel), xmlEscaped(program));
}

QString Autostart::windowsRunCommand(const QString &program) {
    return "\"" + QDir::toNativeSeparators(program) + "\" --background";
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
