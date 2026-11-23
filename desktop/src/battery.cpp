#include "battery.h"

#include "engine.h"

#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusReply>
#include <QDBusVariant>
#include <QJsonObject>
#include <QPointer>

DeviceStatus::DeviceStatus(Settings *settings, Engine *engine, QObject *parent)
    : QObject(parent), m_settings(settings), m_engine(engine) {
    m_timer.setInterval(5000);
    connect(&m_timer, &QTimer::timeout, this, &DeviceStatus::refresh);
    m_timer.start();
    refresh();
}

void DeviceStatus::setPlaying(bool playing) {
    if (m_conditions.playing == playing)
        return;
    m_conditions.playing = playing;
    emit changed();
}

bool DeviceStatus::batteryPaused() const {
    if (!m_settings)
        return false;
    return shouldPauseTransfers(m_settings->pauseOnLowBattery(), m_conditions.batteryPercent,
                                m_conditions.charging,
                                m_settings->policy().setting("pauseOnLowBattery").value("batteryPercent").toInt(15));
}

bool DeviceStatus::shouldPauseTransfers(bool pauseOnLowBattery, int batteryPercent, bool charging,
                                        int thresholdPercent) {
    // No battery (percent < 0) or charging: never pause for battery.
    return pauseOnLowBattery && batteryPercent >= 0 && batteryPercent <= thresholdPercent && !charging;
}

DeviceConditions DeviceStatus::conditionsFrom(bool connected, bool wifi, bool metered, bool charging,
                                              int batteryPercent, bool playing) {
    DeviceConditions c;
    c.connected = connected;
    c.wifi = wifi;
    c.metered = metered;
    c.charging = charging;
    c.batteryPercent = batteryPercent;
    c.playing = playing;
    return c;
}

void DeviceStatus::refresh() {
    const DeviceConditions before = m_conditions;
    const bool beforeBattery = m_onBattery;
    const int beforePort = m_listenPort;
    readUPower();
    readNetworkManager();
    if (m_engine && m_engine->ready()) {
        QPointer<DeviceStatus> self(this);
        m_engine->get("/stats", [self](const QJsonObject &stats, int status) {
            if (!self || status != 200)
                return;
            const int port = stats.value("listenPort").toInt();
            if (port != self->m_listenPort) {
                self->m_listenPort = port;
                emit self->changed();
            }
        });
    }
    if (before.connected != m_conditions.connected || before.wifi != m_conditions.wifi ||
        before.metered != m_conditions.metered || before.charging != m_conditions.charging ||
        before.batteryPercent != m_conditions.batteryPercent || beforeBattery != m_onBattery ||
        beforePort != m_listenPort) {
        emit changed();
    }
}

static QVariant dbusProperty(const QString &service, const QString &path, const QString &interface,
                             const QString &name) {
    QDBusInterface iface(service, path, "org.freedesktop.DBus.Properties", QDBusConnection::systemBus());
    if (!iface.isValid())
        return {};
    QDBusReply<QVariant> reply = iface.call("Get", interface, name);
    if (!reply.isValid())
        return {};
    const QVariant value = reply.value();
    // Properties wrap the real value in a QDBusVariant.
    if (value.canConvert<QDBusVariant>())
        return qvariant_cast<QDBusVariant>(value).variant();
    return value;
}

void DeviceStatus::readUPower() {
    const QVariant onBattery = dbusProperty("org.freedesktop.UPower", "/org/freedesktop/UPower",
                                            "org.freedesktop.UPower", "OnBattery");
    const QVariant percentage =
        dbusProperty("org.freedesktop.UPower", "/org/freedesktop/UPower/devices/DisplayDevice",
                     "org.freedesktop.UPower.Device", "Percentage");
    const QVariant isPresent =
        dbusProperty("org.freedesktop.UPower", "/org/freedesktop/UPower/devices/DisplayDevice",
                     "org.freedesktop.UPower.Device", "IsPresent");
    const QVariant type =
        dbusProperty("org.freedesktop.UPower", "/org/freedesktop/UPower/devices/DisplayDevice",
                     "org.freedesktop.UPower.Device", "Type");

    // Type 2 is Battery. Desktops often expose a DisplayDevice that is not a battery.
    m_hasBattery = isPresent.toBool() && (type.isValid() ? type.toUInt() == 2 : percentage.isValid());
    if (!m_hasBattery && !onBattery.isValid()) {
        // No UPower battery: treat as mains power so pause-on-low-battery stays off.
        m_onBattery = false;
        m_conditions.charging = true;
        m_conditions.batteryPercent = -1;
        return;
    }
    m_onBattery = onBattery.isValid() ? onBattery.toBool() : false;
    m_conditions.charging = !m_onBattery;
    if (percentage.isValid() && m_hasBattery)
        m_conditions.batteryPercent = int(percentage.toDouble());
    else
        m_conditions.batteryPercent = m_hasBattery ? 100 : -1;
}

void DeviceStatus::readNetworkManager() {
    const QVariant state = dbusProperty("org.freedesktop.NetworkManager", "/org/freedesktop/NetworkManager",
                                        "org.freedesktop.NetworkManager", "State");
    const QVariant primaryType =
        dbusProperty("org.freedesktop.NetworkManager", "/org/freedesktop/NetworkManager",
                     "org.freedesktop.NetworkManager", "PrimaryConnectionType");
    const QVariant metered = dbusProperty("org.freedesktop.NetworkManager", "/org/freedesktop/NetworkManager",
                                          "org.freedesktop.NetworkManager", "Metered");

    // NM_STATE_CONNECTED_LOCAL=60, SITE=70, GLOBAL=70 in older docs; GLOBAL is 70.
    const uint nmState = state.isValid() ? state.toUInt() : 70;
    m_conditions.connected = !state.isValid() || nmState >= 60;

    const QString type = primaryType.toString();
    // Cable counts as Wi-Fi for "Only on Wi-Fi" / Transfer on Wi-Fi (DeviceConditions).
    const bool wifiLike = type.contains(QLatin1String("wireless")) || type.contains(QLatin1String("ethernet")) ||
                          type == QLatin1String("bond") || type == QLatin1String("vlan") ||
                          type == QLatin1String("bridge") || type.isEmpty();
    m_conditions.wifi = !m_conditions.connected || wifiLike;

    // NM_METERED_YES=1, GUESS_YES=3.
    const uint m = metered.isValid() ? metered.toUInt() : 0;
    m_conditions.metered = (m == 1 || m == 3);
}
