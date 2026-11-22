#pragma once

#include <QObject>
#include <QString>

// Sizes, speeds, and times as the user reads them, in the current locale.
// QML reaches these through the `fmt` context object.
class Format : public QObject {
    Q_OBJECT

public:
    using QObject::QObject;

    Q_INVOKABLE static QString bytes(double value);
    Q_INVOKABLE static QString speed(double bytesPerSecond);
    Q_INVOKABLE static QString percent(double fraction);
    // "1:02:03" or "4:05" from milliseconds.
    Q_INVOKABLE static QString clock(double milliseconds);
    // "3 h 4 min", "5 min", "40 s" from seconds; empty for unknown.
    Q_INVOKABLE static QString eta(double seconds);
};
