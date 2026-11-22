#include "format.h"

#include <QLocale>
#include <cmath>

QString Format::bytes(double value) {
    if (value < 0 || !std::isfinite(value))
        value = 0;
    if (value < 1024)
        return QObject::tr("%1 B").arg(qint64(value));
    return QLocale().formattedDataSize(qint64(value), 1, QLocale::DataSizeTraditionalFormat);
}

QString Format::speed(double bytesPerSecond) {
    return QObject::tr("%1/s").arg(bytes(bytesPerSecond));
}

QString Format::percent(double fraction) {
    if (!std::isfinite(fraction))
        fraction = 0;
    const double value = std::clamp(fraction, 0.0, 1.0) * 100;
    // Whole numbers read better, except the last stretch where 99.6 % is not done.
    const int decimals = value > 0 && value < 100 && value != std::floor(value) && value >= 99 ? 1 : 0;
    const double shown = decimals ? std::floor(value * 10) / 10 : std::floor(value);
    return QObject::tr("%1%").arg(QLocale().toString(shown, 'f', decimals));
}

QString Format::clock(double milliseconds) {
    if (milliseconds < 0 || !std::isfinite(milliseconds))
        milliseconds = 0;
    const qint64 total = qint64(milliseconds / 1000);
    const qint64 h = total / 3600, m = (total / 60) % 60, s = total % 60;
    auto two = [](qint64 v) { return QString::number(v).rightJustified(2, '0'); };
    return h > 0 ? QString("%1:%2:%3").arg(h).arg(two(m), two(s)) : QString("%1:%2").arg(m).arg(two(s));
}

QString Format::eta(double seconds) {
    if (seconds <= 0 || !std::isfinite(seconds))
        return {};
    const qint64 total = qint64(std::ceil(seconds));
    if (total < 60)
        return QObject::tr("%1 s").arg(total);
    const qint64 minutes = (total + 59) / 60;
    if (minutes < 60)
        return QObject::tr("%1 min").arg(minutes);
    const qint64 days = minutes / 1440;
    if (days > 0)
        return QObject::tr("%1 d %2 h").arg(days).arg((minutes % 1440) / 60);
    return QObject::tr("%1 h %2 min").arg(minutes / 60).arg(minutes % 60);
}
