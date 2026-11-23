#include "portalfilepicker.h"

#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusMetaType>
#include <QDBusObjectPath>
#include <QDBusPendingCallWatcher>
#include <QDBusPendingReply>
#include <QRandomGenerator>
#include <QUrl>

namespace {

// The portal's filter type: a(sa(us)), a name and (kind, pattern) pairs,
// where kind 0 is a glob and 1 a MIME type.
struct FilterRule {
    uint kind = 0;
    QString pattern;
};
struct Filter {
    QString name;
    QList<FilterRule> rules;
};
using Filters = QList<Filter>;

QDBusArgument &operator<<(QDBusArgument &argument, const FilterRule &rule) {
    argument.beginStructure();
    argument << rule.kind << rule.pattern;
    argument.endStructure();
    return argument;
}
const QDBusArgument &operator>>(const QDBusArgument &argument, FilterRule &rule) {
    argument.beginStructure();
    argument >> rule.kind >> rule.pattern;
    argument.endStructure();
    return argument;
}
QDBusArgument &operator<<(QDBusArgument &argument, const Filter &filter) {
    argument.beginStructure();
    argument << filter.name << filter.rules;
    argument.endStructure();
    return argument;
}
const QDBusArgument &operator>>(const QDBusArgument &argument, Filter &filter) {
    argument.beginStructure();
    argument >> filter.name >> filter.rules;
    argument.endStructure();
    return argument;
}

}

Q_DECLARE_METATYPE(FilterRule)
Q_DECLARE_METATYPE(Filter)
Q_DECLARE_METATYPE(Filters)

void PortalFilePicker::open(const QString &kind) {
    if (busy())
        return;
    static const bool registered = [] {
        qDBusRegisterMetaType<FilterRule>();
        qDBusRegisterMetaType<QList<FilterRule>>();
        qDBusRegisterMetaType<Filter>();
        qDBusRegisterMetaType<Filters>();
        return true;
    }();
    Q_UNUSED(registered)

    QDBusConnection bus = QDBusConnection::sessionBus();
    if (!bus.isConnected()) {
        emit failed(tr("No desktop session bus, so the file picker can't open."));
        return;
    }
    m_kind = kind;
    const QString token = QString("torrentplayer%1").arg(QRandomGenerator::global()->generate());
    QString sender = bus.baseService().mid(1);
    sender.replace('.', '_');
    // Listen before calling: the portal may answer before the call returns.
    m_requestPath = QString("/org/freedesktop/portal/desktop/request/%1/%2").arg(sender, token);
    bus.connect("org.freedesktop.portal.Desktop", m_requestPath, "org.freedesktop.portal.Request", "Response",
                this, SLOT(handleResponse(uint, QVariantMap)));

    const bool directory = kind == QLatin1String("directory");
    QString title;
    QVariantMap options{{"handle_token", token}, {"modal", true}, {"multiple", false}};
    if (directory) {
        title = tr("Choose save folder");
        options.insert("directory", true); // some portals honour this on OpenFile too
    } else if (kind == "subtitle") {
        title = tr("Open subtitles");
        Filters filters;
        filters << Filter{tr("Subtitles"), {{0, "*.srt"}, {0, "*.vtt"}, {0, "*.SRT"}, {0, "*.VTT"}}};
        options.insert("filters", QVariant::fromValue(filters));
    } else {
        title = tr("Open torrent file");
        Filters filters;
        filters << Filter{tr("Torrent files"), {{0, "*.torrent"}, {1, "application/x-bittorrent"}}};
        options.insert("filters", QVariant::fromValue(filters));
    }

    const char *method = directory ? "OpenDirectory" : "OpenFile";
    QDBusMessage call = QDBusMessage::createMethodCall("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
                                                       "org.freedesktop.portal.FileChooser", method);
    call << QString() << title << options;
    setBusy(true);
    auto *watcher = new QDBusPendingCallWatcher(bus.asyncCall(call), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this, watcher, directory, title, options] {
        watcher->deleteLater();
        QDBusPendingReply<QDBusObjectPath> reply = *watcher;
        if (reply.isError() && directory) {
            // Older portals may lack OpenDirectory; fall back to OpenFile with directory=true.
            QDBusConnection bus = QDBusConnection::sessionBus();
            QDBusMessage fallback = QDBusMessage::createMethodCall(
                "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.FileChooser", "OpenFile");
            fallback << QString() << title << options;
            auto *retry = new QDBusPendingCallWatcher(bus.asyncCall(fallback), this);
            connect(retry, &QDBusPendingCallWatcher::finished, this, [this, retry] {
                retry->deleteLater();
                QDBusPendingReply<QDBusObjectPath> again = *retry;
                if (again.isError()) {
                    setBusy(false);
                    emit failed(tr("The folder picker didn't open: %1").arg(again.error().message()));
                    return;
                }
                const QString path = again.value().path();
                if (path != m_requestPath) {
                    QDBusConnection bus = QDBusConnection::sessionBus();
                    bus.disconnect("org.freedesktop.portal.Desktop", m_requestPath, "org.freedesktop.portal.Request",
                                   "Response", this, SLOT(handleResponse(uint, QVariantMap)));
                    m_requestPath = path;
                    bus.connect("org.freedesktop.portal.Desktop", m_requestPath, "org.freedesktop.portal.Request",
                                "Response", this, SLOT(handleResponse(uint, QVariantMap)));
                }
            });
            return;
        }
        if (reply.isError()) {
            setBusy(false);
            emit failed(tr("The file picker didn't open: %1").arg(reply.error().message()));
            return;
        }
        // Older portals pick their own request path.
        const QString path = reply.value().path();
        if (path != m_requestPath) {
            QDBusConnection bus = QDBusConnection::sessionBus();
            bus.disconnect("org.freedesktop.portal.Desktop", m_requestPath, "org.freedesktop.portal.Request",
                           "Response", this, SLOT(handleResponse(uint, QVariantMap)));
            m_requestPath = path;
            bus.connect("org.freedesktop.portal.Desktop", m_requestPath, "org.freedesktop.portal.Request",
                        "Response", this, SLOT(handleResponse(uint, QVariantMap)));
        }
    });
}

void PortalFilePicker::handleResponse(uint response, const QVariantMap &results) {
    QDBusConnection::sessionBus().disconnect("org.freedesktop.portal.Desktop", m_requestPath,
                                             "org.freedesktop.portal.Request", "Response", this,
                                             SLOT(handleResponse(uint, QVariantMap)));
    setBusy(false);
    // 0 is success, 1 cancelled by the user, 2 anything else.
    if (response != 0) {
        if (response == 2)
            emit failed(tr("The file picker closed with an error."));
        return;
    }
    const QStringList uris = results.value("uris").toStringList();
    if (uris.isEmpty())
        return;
    emit picked(m_kind, QUrl(uris.first()).toLocalFile());
}
