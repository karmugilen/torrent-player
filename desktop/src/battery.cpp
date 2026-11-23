#include "battery.h"

#include "engine.h"

#include <QJsonObject>
#include <QNetworkInformation>
#include <QPointer>

#if TORRENTPLAYER_FREEDESKTOP
#include <QDBusConnection>
#include <QDBusInterface>
#include <QDBusReply>
#include <QDBusVariant>
#endif

#ifdef Q_OS_WIN
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#endif

#ifdef Q_OS_MACOS
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/ps/IOPSKeys.h>
#include <IOKit/ps/IOPowerSources.h>
#endif

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
    readPower();
    readNetwork();
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

DeviceStatus::Power DeviceStatus::windowsPower(int acLineStatus, int batteryFlag, int lifePercent) {
    Power power;
    // BatteryFlag 128: no system battery; 255: unknown.
    power.hasBattery = batteryFlag != 255 && !(batteryFlag & 128) && lifePercent >= 0 && lifePercent <= 100;
    if (!power.hasBattery)
        return power;
    power.onBattery = acLineStatus == 0;
    power.percent = lifePercent;
    return power;
}

DeviceStatus::Power DeviceStatus::macPower(bool batteryPresent, bool onAC, int currentCapacity, int maxCapacity) {
    Power power;
    power.hasBattery = batteryPresent && maxCapacity > 0;
    if (!power.hasBattery)
        return power;
    power.onBattery = !onAC;
    power.percent = qBound(0, int(qint64(currentCapacity) * 100 / maxCapacity), 100);
    return power;
}

DeviceStatus::Network DeviceStatus::networkFrom(int reachability, int transportMedium, bool metered) {
    Network network;
    if (reachability < 0)
        return network; // No backend: assume online, unmetered, like Linux without NetworkManager.
    network.connected = reachability != int(QNetworkInformation::Reachability::Disconnected);
    const bool wifiLike = transportMedium != int(QNetworkInformation::TransportMedium::Cellular) &&
                          transportMedium != int(QNetworkInformation::TransportMedium::Bluetooth);
    network.wifi = !network.connected || wifiLike;
    network.metered = metered;
    return network;
}

void DeviceStatus::applyPower(const Power &power) {
    m_hasBattery = power.hasBattery;
    m_onBattery = power.hasBattery && power.onBattery;
    m_conditions.charging = !m_onBattery;
    m_conditions.batteryPercent = power.hasBattery ? power.percent : -1;
}

void DeviceStatus::readPower() {
#if TORRENTPLAYER_FREEDESKTOP
    readUPower();
#elif defined(Q_OS_WIN)
    SYSTEM_POWER_STATUS status;
    if (GetSystemPowerStatus(&status))
        applyPower(windowsPower(status.ACLineStatus, status.BatteryFlag, status.BatteryLifePercent));
    else
        applyPower({});
#elif defined(Q_OS_MACOS)
    Power power;
    if (CFTypeRef info = IOPSCopyPowerSourcesInfo()) {
        if (CFArrayRef sources = IOPSCopyPowerSourcesList(info)) {
            for (CFIndex i = 0; i < CFArrayGetCount(sources); ++i) {
                CFDictionaryRef source = IOPSGetPowerSourceDescription(info, CFArrayGetValueAtIndex(sources, i));
                if (!source)
                    continue;
                auto type = static_cast<CFStringRef>(CFDictionaryGetValue(source, CFSTR(kIOPSTypeKey)));
                if (!type || CFStringCompare(type, CFSTR(kIOPSInternalBatteryType), 0) != kCFCompareEqualTo)
                    continue;
                auto state = static_cast<CFStringRef>(CFDictionaryGetValue(source, CFSTR(kIOPSPowerSourceStateKey)));
                auto present = static_cast<CFBooleanRef>(CFDictionaryGetValue(source, CFSTR(kIOPSIsPresentKey)));
                int current = 0, max = 0;
                if (auto n = static_cast<CFNumberRef>(CFDictionaryGetValue(source, CFSTR(kIOPSCurrentCapacityKey))))
                    CFNumberGetValue(n, kCFNumberIntType, &current);
                if (auto n = static_cast<CFNumberRef>(CFDictionaryGetValue(source, CFSTR(kIOPSMaxCapacityKey))))
                    CFNumberGetValue(n, kCFNumberIntType, &max);
                const bool onAC = !state || CFStringCompare(state, CFSTR(kIOPSACPowerValue), 0) == kCFCompareEqualTo;
                power = macPower(!present || CFBooleanGetValue(present), onAC, current, max);
                break;
            }
            CFRelease(sources);
        }
        CFRelease(info);
    }
    applyPower(power);
#else
    applyPower({});
#endif
}

void DeviceStatus::readNetwork() {
#if TORRENTPLAYER_FREEDESKTOP
    readNetworkManager();
#else
    static const bool loaded = QNetworkInformation::loadDefaultBackend();
    QNetworkInformation *info = loaded ? QNetworkInformation::instance() : nullptr;
    const Network network = info ? networkFrom(int(info->reachability()), int(info->transportMedium()), info->isMetered())
                                 : networkFrom(-1, 0, false);
    m_conditions.connected = network.connected;
    m_conditions.wifi = network.wifi;
    m_conditions.metered = network.metered;
#endif
}

#if TORRENTPLAYER_FREEDESKTOP
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
#endif // TORRENTPLAYER_FREEDESKTOP
