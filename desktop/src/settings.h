#pragma once

#include <QJsonObject>
#include <QObject>
#include <QSet>
#include <QTimer>
#include <QVariant>
#include <functional>

#include "engine.h"
#include "policy.h"

// The user's settings (context property `settings`). Defaults and ranges come from
// shared/policy.json; keys are the Android preference keys. Saved atomically to
// ~/.local/share/torrentplayer/settings.json shortly after each change.
class Settings : public QObject {
    Q_OBJECT
    // Downloads: a new torrent starts as Direct Play (true) or a download (false).
    Q_PROPERTY(bool directPlayDefault READ directPlayDefault WRITE setDirectPlayDefault NOTIFY changed)
    // Playback: minutes before an idle Direct Play session is dropped; 0 is off.
    Q_PROPERTY(int idleDropMinutes READ idleDropMinutes WRITE setIdleDropMinutes NOTIFY changed)
    // true: built-in player; false: open mpv in its own window (desktop).
    Q_PROPERTY(bool builtInPlayer READ builtInPlayer WRITE setBuiltInPlayer NOTIFY changed)
    Q_PROPERTY(bool reducedEffects READ reducedEffects WRITE setReducedEffects NOTIFY changed)
    // Desktop save folder for exports; empty means the default Downloads/Torrent Player.
    Q_PROPERTY(QString downloadFolder READ downloadFolder WRITE setDownloadFolder NOTIFY changed)
    // Network: "ANY", "WIFI_ONLY" or "UNMETERED".
    Q_PROPERTY(QString networkPolicy READ networkPolicy WRITE setNetworkPolicy NOTIFY changed)
    // 0 (no limit), 1024, 4096 or 8192.
    Q_PROPERTY(int downloadRateKiB READ downloadRateKiB WRITE setDownloadRateKiB NOTIFY changed)
    Q_PROPERTY(int maxPeers READ maxPeers WRITE setMaxPeers NOTIFY changed)
    Q_PROPERTY(bool antiDpiEnabled READ antiDpiEnabled WRITE setAntiDpiEnabled NOTIFY changed)
    // Seeding.
    Q_PROPERTY(bool seedingEnabled READ seedingEnabled WRITE setSeedingEnabled NOTIFY changed)
    Q_PROPERTY(int seedRatioMilli READ seedRatioMilli WRITE setSeedRatioMilli NOTIFY changed)
    Q_PROPERTY(int seedTimeMinutes READ seedTimeMinutes WRITE setSeedTimeMinutes NOTIFY changed)
    Q_PROPERTY(int seedUploadKiB READ seedUploadKiB WRITE setSeedUploadKiB NOTIFY changed)
    Q_PROPERTY(bool seedWiFiOnly READ seedWiFiOnly WRITE setSeedWiFiOnly NOTIFY changed)
    Q_PROPERTY(bool seedChargingOnly READ seedChargingOnly WRITE setSeedChargingOnly NOTIFY changed)
    // Battery and memory.
    Q_PROPERTY(bool pauseOnLowBattery READ pauseOnLowBattery WRITE setPauseOnLowBattery NOTIFY changed)
    // Auto sizes memory per torrent from free RAM; setting ramCacheMiB turns it off.
    Q_PROPERTY(bool ramCacheAuto READ ramCacheAuto WRITE setRamCacheAuto NOTIFY changed)
    // Memory per torrent in use: the engine's answer, else the auto or chosen size.
    Q_PROPERTY(int ramCacheMiB READ ramCacheMiB WRITE setRamCacheMiB NOTIFY changed)
    Q_PROPERTY(qint64 ramCacheBytes READ ramCacheBytes NOTIFY changed)
    // Slider stops for ramCacheMiB.
    Q_PROPERTY(QVariantList ramCacheSteps READ ramCacheSteps CONSTANT)

public:
    // path: the JSON file to load and save (defaultPath() in the app).
    explicit Settings(const QString &path, const Policy &policy = Policy::instance(), QObject *parent = nullptr);
    ~Settings() override;

    static QString defaultPath();

    // Generic access by key; setValue normalizes, saves and notifies.
    QVariant value(const QString &key) const { return m_values.value(key); }
    void setValue(const QString &key, const QVariant &value);
    QStringList keys() const { return m_policy.settingKeys("desktop"); }
    const Policy &policy() const { return m_policy; }

    bool directPlayDefault() const { return value("directPlayDefault").toBool(); }
    void setDirectPlayDefault(bool v) { setValue("directPlayDefault", v); }
    int idleDropMinutes() const { return value("idleDropMinutes").toInt(); }
    void setIdleDropMinutes(int v) { setValue("idleDropMinutes", v); }
    bool builtInPlayer() const { return value("builtInPlayer").toBool(); }
    void setBuiltInPlayer(bool v) { setValue("builtInPlayer", v); }
    bool reducedEffects() const { return value("reducedEffects").toBool(); }
    void setReducedEffects(bool v) { setValue("reducedEffects", v); }
    QString downloadFolder() const { return m_downloadFolder; }
    void setDownloadFolder(const QString &path);
    QString networkPolicy() const { return value("networkPolicy").toString(); }
    void setNetworkPolicy(const QString &v) { setValue("networkPolicy", v); }
    int downloadRateKiB() const { return value("downloadRateKiB").toInt(); }
    void setDownloadRateKiB(int v) { setValue("downloadRateKiB", v); }
    int maxPeers() const { return value("maxPeers").toInt(); }
    void setMaxPeers(int v) { setValue("maxPeers", v); }
    bool antiDpiEnabled() const { return value("antiDpiEnabled").toBool(); }
    void setAntiDpiEnabled(bool v) { setValue("antiDpiEnabled", v); }
    bool seedingEnabled() const { return value("seedingEnabled").toBool(); }
    void setSeedingEnabled(bool v) { setValue("seedingEnabled", v); }
    int seedRatioMilli() const { return value("seedRatioMilli").toInt(); }
    void setSeedRatioMilli(int v) { setValue("seedRatioMilli", v); }
    int seedTimeMinutes() const { return value("seedTimeMinutes").toInt(); }
    void setSeedTimeMinutes(int v) { setValue("seedTimeMinutes", v); }
    int seedUploadKiB() const { return value("seedUploadKiB").toInt(); }
    void setSeedUploadKiB(int v) { setValue("seedUploadKiB", v); }
    bool seedWiFiOnly() const { return value("seedWiFiOnly").toBool(); }
    void setSeedWiFiOnly(bool v) { setValue("seedWiFiOnly", v); }
    bool seedChargingOnly() const { return value("seedChargingOnly").toBool(); }
    void setSeedChargingOnly(bool v) { setValue("seedChargingOnly", v); }
    bool pauseOnLowBattery() const { return value("pauseOnLowBattery").toBool(); }
    void setPauseOnLowBattery(bool v) { setValue("pauseOnLowBattery", v); }

    bool ramCacheAuto() const { return value("ramCacheAuto").toBool(); }
    void setRamCacheAuto(bool v);
    int ramCacheMiB() const { return m_appliedMiB > 0 ? m_appliedMiB : wantedRamCacheMiB(); }
    // A chosen size (clamped to the policy range); turns Auto off, as on Android.
    void setRamCacheMiB(int mib);
    qint64 ramCacheBytes() const { return qint64(ramCacheMiB()) * 1024 * 1024; }
    QVariantList ramCacheSteps() const;
    // The size to ask the engine for: the Auto size or the chosen one.
    int wantedRamCacheMiB() const { return ramCacheAuto() ? m_autoMiB : value("ramCacheMiB").toInt(); }
    // What the engine applied (its /settings reply), shown as ramCacheMiB.
    void setAppliedRamCacheMiB(int mib);

    // Torrents running now (not paused, unfinished or seeding); Auto shares memory among them.
    Q_INVOKABLE void setActiveTransfers(int count);
    int activeTransfers() const { return m_activeTransfers; }
    // Recomputes the Auto size from /proc/meminfo (call before configuring a torrent).
    Q_INVOKABLE void refreshAutoRamCache();

    Q_INVOKABLE QVariant defaultValue(const QString &key) const { return m_policy.defaultValue(key); }
    Q_INVOKABLE QVariantList options(const QString &key) const { return m_policy.options(key); }
    // Back to the policy defaults for every setting.
    Q_INVOKABLE void reset();
    // Writes the file now (changes are otherwise saved shortly after).
    bool save();

signals:
    void changed();
    // One setting changed (also "ramCacheMiB" when the Auto size moves).
    void settingChanged(const QString &key);

private:
    void load();
    void scheduleSave();

    const Policy m_policy;
    QString m_path;
    QVariantMap m_values;
    QString m_downloadFolder;
    QTimer m_saveTimer;
    int m_autoMiB = 64;
    int m_appliedMiB = 0;
    int m_activeTransfers = 1;
};

// Reads MemTotal and MemAvailable (bytes) from /proc/meminfo; false when unreadable.
bool readMemInfo(qint64 &totalBytes, qint64 &availableBytes, const QString &path = "/proc/meminfo");

struct EngineRequest {
    QString path;
    QJsonObject body;
    bool operator==(const EngineRequest &o) const { return path == o.path && body == o.body; }
};

// What the device is doing, for the seeding and transfer gates Android applies.
struct DeviceConditions {
    bool connected = true;
    bool wifi = true;      // desktops on a cable count as Wi-Fi for "Only on Wi-Fi"
    bool metered = false;
    bool charging = true;  // no battery means mains power
    int batteryPercent = -1;
    bool playing = false;  // a video is open: seeding pauses
};

// Sends the settings to the engine the way Android's LibrarySession does: every
// request on engine start, and the affected ones whenever a setting changes.
class EngineSettings : public QObject {
    Q_OBJECT

public:
    using Post = std::function<void(const QString &path, const QJsonObject &body, Engine::Reply reply)>;

    EngineSettings(Engine *engine, Settings *settings, QObject *parent = nullptr);
    // For tests: post goes to a fake.
    EngineSettings(Settings *settings, Post post, QObject *parent = nullptr);

    DeviceConditions conditions() const { return m_conditions; }
    void setConditions(const DeviceConditions &conditions);

    // The requests Android sends after the engine starts, in its order.
    static QList<EngineRequest> startRequests(const Settings &settings, const DeviceConditions &conditions);
    // The requests that bring the engine in line after one setting changed.
    static QList<EngineRequest> requestsFor(const QString &key, const Settings &settings, const DeviceConditions &conditions);

    static bool networkAllowed(const Settings &settings, const DeviceConditions &conditions);
    static bool batteryPaused(const Settings &settings, const DeviceConditions &conditions);
    static bool seedingAllowed(const Settings &settings, const DeviceConditions &conditions);

    // Sends every start request now.
    void applyAll();

private:
    void send(const QList<EngineRequest> &requests);
    void flush();

    Settings *m_settings;
    Post m_post;
    DeviceConditions m_conditions;
    QSet<QString> m_pending;
    QTimer m_flushTimer;
};
