#pragma once

#include <QObject>
#include <QTimer>

#include "settings.h"

class Engine;

// Reads power and network over D-Bus (UPower + NetworkManager) so seeding and
// transfer rules match Android. Machines without a battery report charging with
// batteryPercent -1, so "Pause on low battery" does nothing.
class DeviceStatus : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool connected READ connected NOTIFY changed)
    Q_PROPERTY(bool wifi READ wifi NOTIFY changed)
    Q_PROPERTY(bool metered READ metered NOTIFY changed)
    Q_PROPERTY(bool charging READ charging NOTIFY changed)
    Q_PROPERTY(int batteryPercent READ batteryPercent NOTIFY changed)
    Q_PROPERTY(bool onBattery READ onBattery NOTIFY changed)
    Q_PROPERTY(bool playing READ playing WRITE setPlaying NOTIFY changed)
    Q_PROPERTY(bool batteryPaused READ batteryPaused NOTIFY changed)
    Q_PROPERTY(int listenPort READ listenPort NOTIFY changed)

public:
    DeviceStatus(Settings *settings, Engine *engine = nullptr, QObject *parent = nullptr);

    bool connected() const { return m_conditions.connected; }
    bool wifi() const { return m_conditions.wifi; }
    bool metered() const { return m_conditions.metered; }
    bool charging() const { return m_conditions.charging; }
    int batteryPercent() const { return m_conditions.batteryPercent; }
    bool onBattery() const { return m_onBattery; }
    bool playing() const { return m_conditions.playing; }
    void setPlaying(bool playing);
    bool batteryPaused() const;
    int listenPort() const { return m_listenPort; }

    DeviceConditions conditions() const { return m_conditions; }

    // Pure helpers for tests (threshold from policy pauseOnLowBattery.batteryPercent).
    static bool shouldPauseTransfers(bool pauseOnLowBattery, int batteryPercent, bool charging,
                                     int thresholdPercent = 15);
    static DeviceConditions conditionsFrom(bool connected, bool wifi, bool metered, bool charging,
                                           int batteryPercent, bool playing);

public slots:
    void refresh();

signals:
    void changed();

private:
    void readUPower();
    void readNetworkManager();

    Settings *m_settings = nullptr;
    Engine *m_engine = nullptr;
    DeviceConditions m_conditions;
    bool m_onBattery = false;
    bool m_hasBattery = false;
    int m_listenPort = 0;
    QTimer m_timer;
};
