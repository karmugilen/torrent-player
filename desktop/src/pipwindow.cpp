#include "pipwindow.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QProcess>
#include <QScreen>
#include <QtMath>

PipWindow::PipWindow(QObject *parent) : QObject(parent) {}

PipWindow::~PipWindow() {
    // Don't leave the window pinned on every workspace when the app goes.
    if (m_active)
        leave();
}

QSize PipWindow::pipSize(double aspect, QSize box) {
    if (!(aspect > 0.0) || !qIsFinite(aspect))
        aspect = 16.0 / 9.0;
    // Keep weird stream metadata from making a sliver.
    aspect = qBound(0.5, aspect, 3.0);
    int width = box.width();
    int height = qRound(width / aspect);
    if (height > box.height()) {
        height = box.height();
        width = qRound(height * aspect);
    }
    return QSize(width, height);
}

QPoint PipWindow::cornerPosition(const QRect &area, const QSize &size, int margin) {
    const int x = area.x() + area.width() - margin - size.width();
    const int y = area.y() + area.height() - margin - size.height();
    return QPoint(qMax(area.x(), x), qMax(area.y(), y));
}

PipWindow::HyprClient PipWindow::findClient(const QByteArray &clientsJson, qint64 pid) {
    const QJsonArray clients = QJsonDocument::fromJson(clientsJson).array();
    for (const QJsonValue &value : clients) {
        const QJsonObject c = value.toObject();
        if (qint64(c.value("pid").toDouble(-1)) != pid)
            continue;
        if (c.contains("mapped") && !c.value("mapped").toBool())
            continue;
        if (c.value("hidden").toBool())
            continue;
        HyprClient client;
        client.address = c.value("address").toString();
        client.floating = c.value("floating").toBool();
        client.pinned = c.value("pinned").toBool();
        client.monitor = c.value("monitor").toInt(-1);
        client.fullscreen = c.value("fullscreen").toInt();
        const QJsonArray at = c.value("at").toArray();
        const QJsonArray size = c.value("size").toArray();
        client.geometry = QRect(at.at(0).toInt(), at.at(1).toInt(), size.at(0).toInt(), size.at(1).toInt());
        if (client.isValid())
            return client;
    }
    return {};
}

QRect PipWindow::monitorArea(const QByteArray &monitorsJson, int monitorId) {
    const QJsonArray monitors = QJsonDocument::fromJson(monitorsJson).array();
    QJsonObject chosen;
    for (const QJsonValue &value : monitors) {
        const QJsonObject m = value.toObject();
        if (m.value("id").toInt(-2) == monitorId) {
            chosen = m;
            break;
        }
        if (chosen.isEmpty() && m.value("focused").toBool())
            chosen = m;
    }
    if (chosen.isEmpty() && !monitors.isEmpty())
        chosen = monitors.first().toObject();
    if (chosen.isEmpty())
        return {};
    double scale = chosen.value("scale").toDouble(1.0);
    if (scale <= 0)
        scale = 1.0;
    int width = qRound(chosen.value("width").toDouble() / scale);
    int height = qRound(chosen.value("height").toDouble() / scale);
    // Transforms 1, 3, 5 and 7 turn the monitor on its side.
    if (chosen.value("transform").toInt() % 2 == 1)
        std::swap(width, height);
    const QJsonArray reserved = chosen.value("reserved").toArray(); // left, top, right, bottom
    const int left = reserved.at(0).toInt(), top = reserved.at(1).toInt();
    const int right = reserved.at(2).toInt(), bottom = reserved.at(3).toInt();
    return QRect(chosen.value("x").toInt() + left, chosen.value("y").toInt() + top,
                 width - left - right, height - top - bottom);
}

QStringList PipWindow::enterCommands(const HyprClient &client, const QSize &size, const QPoint &position) {
    if (!client.isValid())
        return {};
    const QString target = "address:" + client.address;
    QStringList commands;
    if (client.fullscreen != 0)
        commands << "fullscreenstate 0 0";
    if (!client.floating)
        commands << "setfloating " + target;
    commands << QString("resizewindowpixel exact %1 %2,%3").arg(size.width()).arg(size.height()).arg(target);
    commands << QString("movewindowpixel exact %1 %2,%3").arg(position.x()).arg(position.y()).arg(target);
    if (!client.pinned)
        commands << "pin " + target;
    return commands;
}

QStringList PipWindow::leaveCommands(const HyprClient &before, const HyprClient &now) {
    // The window is gone (or was never found): nothing to put back.
    if (!now.isValid())
        return {};
    const QString target = "address:" + now.address;
    QStringList commands;
    // pin toggles, so it is only sent when the pinned state differs.
    if (now.pinned != before.pinned)
        commands << "pin " + target;
    if (!before.floating) {
        commands << "settiled " + target;
    } else if (before.geometry.isValid()) {
        commands << QString("resizewindowpixel exact %1 %2,%3")
                        .arg(before.geometry.width()).arg(before.geometry.height()).arg(target);
        commands << QString("movewindowpixel exact %1 %2,%3")
                        .arg(before.geometry.x()).arg(before.geometry.y()).arg(target);
    }
    return commands;
}

QString PipWindow::batch(const QStringList &commands) {
    QStringList parts;
    for (const QString &command : commands)
        parts << "dispatch " + command;
    return parts.join(" ; ");
}

bool PipWindow::onHyprland() {
    return !qEnvironmentVariableIsEmpty("HYPRLAND_INSTANCE_SIGNATURE")
           && QGuiApplication::platformName().startsWith("wayland");
}

QByteArray PipWindow::hyprctl(const QStringList &arguments) {
    QProcess process;
    process.start("hyprctl", arguments);
    if (!process.waitForFinished(2000)) {
        process.kill();
        process.waitForFinished(500);
        qWarning() << "pip: hyprctl did not answer" << arguments;
        return {};
    }
    return process.readAllStandardOutput();
}

void PipWindow::enter(QWindow *window, double aspect) {
    if (m_active || !window)
        return;
    m_window = window;
    m_geometry = window->geometry();
    m_states = window->windowStates();
    m_stayedOnTop = window->flags().testFlag(Qt::WindowStaysOnTopHint);
    m_hyprland = onHyprland();
    const QSize size = pipSize(aspect);

    // Chrome goes first so the window may shrink below its normal minimum.
    m_active = true;
    emit activeChanged();

    if (m_states & (Qt::WindowFullScreen | Qt::WindowMaximized))
        window->showNormal();

    if (m_hyprland) {
        const qint64 pid = QCoreApplication::applicationPid();
        m_before = findClient(hyprctl({"-j", "clients"}), pid);
        if (m_before.isValid()) {
            const QRect area = monitorArea(hyprctl({"-j", "monitors"}), m_before.monitor);
            const QPoint position = area.isValid() ? cornerPosition(area, size) : QPoint(kMargin, kMargin);
            hyprctl({"--batch", batch(enterCommands(m_before, size, position))});
            return;
        }
        qWarning() << "pip: this window is not in hyprctl clients; using Qt instead";
        m_hyprland = false;
    }

    window->setFlag(Qt::WindowStaysOnTopHint, true);
    window->resize(size);
    if (QScreen *screen = window->screen())
        window->setPosition(cornerPosition(screen->availableGeometry(), size));
    window->show();
}

void PipWindow::leave() {
    if (!m_active)
        return;
    m_active = false;
    QWindow *window = m_window.data();

    if (m_hyprland) {
        const HyprClient now = findClient(hyprctl({"-j", "clients"}), QCoreApplication::applicationPid());
        const QStringList commands = leaveCommands(m_before, now);
        if (!commands.isEmpty())
            hyprctl({"--batch", batch(commands)});
    } else if (window) {
        window->setFlag(Qt::WindowStaysOnTopHint, m_stayedOnTop);
        window->setGeometry(m_geometry);
        window->show();
    }
    if (window) {
        if (m_states & Qt::WindowFullScreen)
            window->showFullScreen();
        else if (m_states & Qt::WindowMaximized)
            window->showMaximized();
    }
    emit activeChanged();
}
