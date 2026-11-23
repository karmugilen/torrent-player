#pragma once

#include <QObject>
#include <QString>

// Writes or removes ~/.config/autostart/torrentplayer.desktop so the app can
// start hidden in the tray with --background.
class Autostart : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged)

public:
    // directory: the autostart folder (default: ~/.config/autostart).
    explicit Autostart(const QString &directory = QString(), QObject *parent = nullptr);

    bool enabled() const;
    void setEnabled(bool enabled);

    QString directory() const { return m_directory; }
    QString desktopFilePath() const;

    // Pure helpers for tests.
    static QString desktopFileContents(const QString &execLine = QStringLiteral("torrentplayer --background"));
    static bool writeDesktopFile(const QString &path, const QString &contents);
    static bool removeDesktopFile(const QString &path);

signals:
    void enabledChanged();

private:
    QString m_directory;
};
