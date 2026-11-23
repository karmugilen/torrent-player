#pragma once

#include <QObject>
#include <QString>

// Starts the app at login, hidden in the tray with --background. Off by default.
// Linux: ~/.config/autostart/torrentplayer.desktop. macOS: a LaunchAgent,
// ~/Library/LaunchAgents/app.torrentplayer.desktop.plist. Windows: a value in
// HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
class Autostart : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged)

public:
    // directory: the autostart folder (default: ~/.config/autostart, or
    // ~/Library/LaunchAgents on macOS). A directory given on Windows also means a file
    // there instead of the registry (tests).
    explicit Autostart(const QString &directory = QString(), QObject *parent = nullptr);

    bool enabled() const;
    void setEnabled(bool enabled);

    QString directory() const { return m_directory; }
    // The file that turns autostart on (the .desktop file, or the plist on macOS).
    QString desktopFilePath() const;

    // Pure helpers for tests.
    static QString desktopFileContents(const QString &execLine = QStringLiteral("torrentplayer --background"));
    static QString launchAgentContents(const QString &program);
    static QString windowsRunCommand(const QString &program);
    static bool writeDesktopFile(const QString &path, const QString &contents);
    static bool removeDesktopFile(const QString &path);

    static constexpr const char *launchAgentLabel = "app.torrentplayer.desktop";

signals:
    void enabledChanged();

private:
    bool useRegistry() const;

    QString m_directory;
    bool m_customDirectory = false;
};
