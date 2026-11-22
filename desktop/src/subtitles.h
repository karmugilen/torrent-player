#pragma once

#include <QNetworkAccessManager>
#include <QObject>
#include <QVector>

// External subtitles (SubRip and WebVTT) drawn over the video. Embedded
// tracks are rendered by Qt Multimedia itself; these come from subtitle files
// inside the torrent or picked from disk.
class Subtitles : public QObject {
    Q_OBJECT
    Q_PROPERTY(QString label READ label NOTIFY loadedChanged)
    Q_PROPERTY(bool loaded READ loaded NOTIFY loadedChanged)
    Q_PROPERTY(QString error READ error NOTIFY loadedChanged)

public:
    struct Cue {
        qint64 start = 0;
        qint64 end = 0;
        QString text;
    };

    explicit Subtitles(QObject *parent = nullptr);

    static QVector<Cue> parse(const QByteArray &data);
    void setCues(const QVector<Cue> &cues, const QString &label);
    const QVector<Cue> &cues() const { return m_cues; }

    QString label() const { return m_label; }
    bool loaded() const { return !m_cues.isEmpty(); }
    QString error() const { return m_error; }

    // Loads from a local path or an http URL (the engine's stream server).
    Q_INVOKABLE void load(const QString &source, const QString &label);
    Q_INVOKABLE void clear();
    // Text to show at a position, empty between cues. Plain text: tags are stripped.
    Q_INVOKABLE QString textAt(qint64 positionMs) const;

signals:
    void loadedChanged();

private:
    QVector<Cue> m_cues;
    QString m_label;
    QString m_error;
    QNetworkAccessManager m_network;
    int m_generation = 0;
};
