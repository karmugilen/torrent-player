#include "playerstats.h"

#include "format.h"

#include <QStringList>

QString PlayerStats::line(bool directPlay, double fileProgress, int peers, double downloadSpeed) {
    QStringList parts;
    // Direct Play keeps video in RAM, so a percentage means nothing there.
    if (directPlay)
        parts << tr("Direct Play · streaming from RAM");
    else if (fileProgress >= 0)
        parts << tr("%1% downloaded").arg(int(qBound(0.0, fileProgress, 1.0) * 100));
    if (peers >= 0)
        parts << tr("%n peers", "", peers) + " · " + Format::speed(downloadSpeed);
    return parts.join(QStringLiteral("  ·  "));
}

QString PlayerStats::buffered(qint64 positionMs, qint64 cacheEndMs) {
    const qint64 ahead = qMax<qint64>(0, cacheEndMs - positionMs);
    return ahead >= 1000 ? tr("buffered %1").arg(Format::clock(double(ahead))) : QString();
}

QString PlayerStats::fileLine(double fileProgress, double fileLength, double uploadSpeed) {
    if (fileLength <= 0 || fileProgress < 0)
        return {};
    const double progress = qBound(0.0, fileProgress, 1.0);
    QString text = tr("File %1% · %2 of %3")
                       .arg(int(100 * progress))
                       .arg(Format::bytes(fileLength * progress), Format::bytes(fileLength));
    if (uploadSpeed > 0)
        text += " · ↑ " + Format::speed(uploadSpeed);
    return text;
}
