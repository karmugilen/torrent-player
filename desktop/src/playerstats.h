#pragma once

#include <QObject>
#include <QString>

// The player's stats lines, worded as Android's player (PlayerActivity top info line,
// "buffered" next to the time, Formatters.kt playbackStatsLine). Created in QML as
// PlayerStats; the functions are pure and tested.
class PlayerStats : public QObject {
    Q_OBJECT

public:
    using QObject::QObject;

    // "Direct Play · streaming from RAM  ·  3 peers · 1.2 MB/s" or "42% downloaded  ·  …".
    // fileProgress < 0 is unknown; peers < 0 leaves the swarm part out.
    Q_INVOKABLE static QString line(bool directPlay, double fileProgress, int peers, double downloadSpeed);
    // "buffered 1:23" while mpv holds at least a second ahead, else "".
    Q_INVOKABLE static QString buffered(qint64 positionMs, qint64 cacheEndMs);
    // "File 42% · 120 MB of 1.2 GB · ↑ 50 KB/s"; "" until there is something to show.
    Q_INVOKABLE static QString fileLine(double fileProgress, double fileLength, double uploadSpeed);
};
