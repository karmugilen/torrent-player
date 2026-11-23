#include "links.h"

#include <QFileInfo>
#include <QRegularExpression>
#include <QUrl>
#include <QUrlQuery>

namespace Links {

static QString base32ToHex(const QString &text) {
    static const QString alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    QByteArray bytes;
    quint64 buffer = 0;
    int bits = 0;
    for (const QChar c : text.toUpper()) {
        const int value = alphabet.indexOf(c);
        if (value < 0)
            return {};
        buffer = (buffer << 5) | quint64(value);
        bits += 5;
        if (bits >= 8) {
            bits -= 8;
            bytes.append(char((buffer >> bits) & 0xff));
        }
    }
    return QString::fromLatin1(bytes.toHex());
}

QString magnetInfoHash(const QString &magnet) {
    const QUrl url(magnet.trimmed());
    if (url.scheme().compare("magnet", Qt::CaseInsensitive) != 0)
        return {};
    const QUrlQuery query(url);
    for (const auto &item : query.queryItems(QUrl::FullyDecoded)) {
        if (item.first != "xt")
            continue;
        const QString value = item.second;
        static const QRegularExpression hex40("^urn:btih:([0-9a-fA-F]{40})$");
        static const QRegularExpression b32("^urn:btih:([A-Za-z2-7]{32})$");
        static const QRegularExpression btmh("^urn:btmh:1220([0-9a-fA-F]{64})$");
        if (auto m = hex40.match(value); m.hasMatch())
            return m.captured(1).toLower();
        if (auto m = b32.match(value); m.hasMatch())
            return base32ToHex(m.captured(1));
        if (auto m = btmh.match(value); m.hasMatch())
            return m.captured(1).toLower();
    }
    return {};
}

QString localPath(const QString &input) {
    const QString text = input.trimmed();
    if (text.startsWith("file:", Qt::CaseInsensitive))
        return QUrl(text).toLocalFile();
    return text;
}

Kind classify(const QString &input) {
    const QString text = input.trimmed();
    if (text.isEmpty())
        return Kind::Invalid;
    if (text.startsWith("magnet:", Qt::CaseInsensitive))
        return magnetInfoHash(text).isEmpty() ? Kind::Invalid : Kind::Magnet;
    if (text.startsWith("http://", Qt::CaseInsensitive) || text.startsWith("https://", Qt::CaseInsensitive))
        return QUrl(text).isValid() ? Kind::TorrentUrl : Kind::Invalid;
    const QFileInfo info(localPath(text));
    if (info.isFile())
        return Kind::TorrentFile;
    return Kind::Invalid;
}

static QString suffix(const QString &name) {
    return QFileInfo(name).suffix().toLower();
}

bool isVideo(const QString &name, const QString &mimeType) {
    static const QStringList extensions{"mp4", "mkv", "webm", "avi", "mov", "m4v", "ts", "m2ts",
                                        "mpg", "mpeg", "wmv", "flv", "ogv", "3gp"};
    return mimeType.startsWith("video/") || extensions.contains(suffix(name));
}

bool isSubtitle(const QString &name) {
    static const QStringList extensions{"srt", "vtt", "ass", "ssa", "sub"};
    return extensions.contains(suffix(name));
}

bool isImage(const QString &name) {
    static const QStringList extensions{"jpg", "jpeg", "png", "webp"};
    return extensions.contains(suffix(name));
}

bool subtitleMatches(const QString &videoName, const QString &subtitleName) {
    if (!isSubtitle(subtitleName))
        return false;
    const QString base = QFileInfo(videoName).completeBaseName().toLower();
    return !base.isEmpty() && QFileInfo(subtitleName).fileName().toLower().startsWith(base);
}

QString naturalMediaKey(const QString &path) {
    static const QRegularExpression digits("\\d+");
    const QString lower = path.toLower();
    QString out;
    qsizetype last = 0;
    for (auto it = digits.globalMatch(lower); it.hasNext();) {
        const auto m = it.next();
        out += lower.mid(last, m.capturedStart() - last);
        QString number = m.captured();
        while (number.startsWith('0'))
            number.remove(0, 1);
        if (number.isEmpty())
            number = "0";
        out += number.rightJustified(20, '0');
        last = m.capturedEnd();
    }
    return out + lower.mid(last);
}

bool isExtraVideo(const QString &path) {
    static const QRegularExpression extras(
        "(?<![a-z])(sample|trailer|teaser|featurettes?|extras|bonus|behind[ ._-]the[ ._-]scenes|"
        "deleted[ ._-]scenes?|making[ ._-]of|interviews?)(?![a-z])",
        QRegularExpression::CaseInsensitiveOption);
    return extras.match(path).hasMatch();
}

}
