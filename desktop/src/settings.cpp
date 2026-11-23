#include "settings.h"

#include "platform.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QPointer>
#include <QSaveFile>
#include <QStandardPaths>
#include <QTextStream>

#if defined(Q_OS_WIN)
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#elif defined(Q_OS_MACOS)
#include <mach/mach.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#endif

static constexpr qint64 MiB = 1024 * 1024;

Settings::Settings(const QString &path, const Policy &policy, QObject *parent)
    : QObject(parent), m_policy(policy), m_path(path) {
    m_saveTimer.setSingleShot(true);
    m_saveTimer.setInterval(300);
    connect(&m_saveTimer, &QTimer::timeout, this, &Settings::save);
    for (const QString &key : keys())
        m_values.insert(key, m_policy.defaultValue(key));
    m_autoMiB = m_policy.ramRule().minMiB;
    load();
    refreshAutoRamCache();
}

Settings::~Settings() {
    if (m_saveTimer.isActive())
        save();
}

QString Settings::defaultPath() {
#if TORRENTPLAYER_FREEDESKTOP
    return QDir::homePath() + "/.local/share/torrentplayer/settings.json";
#else
    return QStandardPaths::writableLocation(QStandardPaths::AppLocalDataLocation) + "/settings.json";
#endif
}

void Settings::load() {
    QFile file(m_path);
    if (!file.open(QIODevice::ReadOnly))
        return;
    const QJsonObject saved = QJsonDocument::fromJson(file.readAll()).object();
    for (const QString &key : keys())
        if (saved.contains(key))
            m_values.insert(key, m_policy.normalize(key, saved.value(key).toVariant()));
    // As on Android: an older file with a chosen size but no Auto flag keeps that size.
    if (!saved.contains("ramCacheAuto") && saved.contains("ramCacheMiB"))
        m_values.insert("ramCacheAuto", false);
    m_downloadFolder = saved.value("downloadFolder").toString();
}

void Settings::setDownloadFolder(const QString &path) {
    const QString cleaned = path.trimmed();
    if (cleaned == m_downloadFolder)
        return;
    m_downloadFolder = cleaned;
    scheduleSave();
    emit changed();
}

bool Settings::save() {
    m_saveTimer.stop();
    QDir().mkpath(QFileInfo(m_path).absolutePath());
    QJsonObject out;
    for (auto it = m_values.cbegin(); it != m_values.cend(); ++it)
        out.insert(it.key(), QJsonValue::fromVariant(it.value()));
    if (!m_downloadFolder.isEmpty())
        out.insert("downloadFolder", m_downloadFolder);
    QSaveFile file(m_path);
    if (!file.open(QIODevice::WriteOnly))
        return false;
    file.write(QJsonDocument(out).toJson());
    return file.commit();
}

void Settings::scheduleSave() { m_saveTimer.start(); }

void Settings::setValue(const QString &key, const QVariant &value) {
    if (!m_values.contains(key))
        return;
    const QVariant normalized = m_policy.normalize(key, value);
    if (m_values.value(key) == normalized)
        return;
    m_values.insert(key, normalized);
    scheduleSave();
    emit settingChanged(key);
    emit changed();
}

void Settings::setRamCacheAuto(bool v) {
    if (v == ramCacheAuto())
        return;
    if (v)
        refreshAutoRamCache();
    m_appliedMiB = 0;
    setValue("ramCacheAuto", v);
}

void Settings::setRamCacheMiB(int mib) {
    const QVariant size = m_policy.normalize("ramCacheMiB", mib);
    if (!ramCacheAuto() && value("ramCacheMiB") == size)
        return;
    m_values.insert("ramCacheMiB", size);
    m_values.insert("ramCacheAuto", false);
    m_appliedMiB = 0;
    scheduleSave();
    emit settingChanged("ramCacheMiB");
    emit changed();
}

void Settings::setAppliedRamCacheMiB(int mib) {
    if (mib == m_appliedMiB)
        return;
    m_appliedMiB = mib;
    emit changed();
}

QVariantList Settings::ramCacheSteps() const {
    QVariantList steps;
    for (int step : m_policy.ramCacheStepsMiB())
        steps << step;
    return steps;
}

void Settings::setActiveTransfers(int count) {
    count = std::max(1, count);
    if (count == m_activeTransfers)
        return;
    m_activeTransfers = count;
    refreshAutoRamCache();
}

void Settings::refreshAutoRamCache() {
    qint64 total = 0, available = 0;
    if (!readSystemMemory(total, available))
        total = available = 0;
    const int size = autoRamCacheMiB(total, available, m_activeTransfers, m_policy.ramRule());
    if (size == m_autoMiB)
        return;
    m_autoMiB = size;
    if (ramCacheAuto()) {
        m_appliedMiB = 0;
        emit settingChanged("ramCacheMiB");
        emit changed();
    }
}

void Settings::reset() {
    bool any = false;
    for (const QString &key : keys()) {
        const QVariant def = m_policy.defaultValue(key);
        if (m_values.value(key) == def)
            continue;
        m_values.insert(key, def);
        any = true;
        emit settingChanged(key);
    }
    m_appliedMiB = 0;
    refreshAutoRamCache();
    save();
    if (any)
        emit changed();
}

bool readMemInfo(qint64 &totalBytes, qint64 &availableBytes, const QString &path) {
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly | QIODevice::Text))
        return false;
    totalBytes = availableBytes = -1;
    QTextStream in(&file);
    QString line;
    while (in.readLineInto(&line)) {
        const QStringList parts = line.simplified().split(' ');
        if (parts.size() < 2)
            continue;
        const qint64 kib = parts[1].toLongLong();
        if (parts[0] == "MemTotal:")
            totalBytes = kib * 1024;
        else if (parts[0] == "MemAvailable:")
            availableBytes = kib * 1024;
    }
    return totalBytes > 0 && availableBytes >= 0;
}

bool readSystemMemory(qint64 &totalBytes, qint64 &availableBytes) {
#if defined(Q_OS_WIN)
    MEMORYSTATUSEX status;
    status.dwLength = sizeof(status);
    if (!GlobalMemoryStatusEx(&status))
        return false;
    totalBytes = qint64(status.ullTotalPhys);
    availableBytes = qint64(status.ullAvailPhys);
    return totalBytes > 0;
#elif defined(Q_OS_MACOS)
    uint64_t memsize = 0;
    size_t length = sizeof(memsize);
    if (sysctlbyname("hw.memsize", &memsize, &length, nullptr, 0) != 0 || memsize == 0)
        return false;
    totalBytes = qint64(memsize);
    // Free plus reclaimable pages, roughly what Linux calls MemAvailable.
    vm_statistics64_data_t vm;
    mach_msg_type_number_t count = HOST_VM_INFO64_COUNT;
    vm_size_t page = 0;
    if (host_page_size(mach_host_self(), &page) != KERN_SUCCESS ||
        host_statistics64(mach_host_self(), HOST_VM_INFO64, reinterpret_cast<host_info64_t>(&vm), &count) != KERN_SUCCESS)
        return false;
    availableBytes = qint64(vm.free_count + vm.inactive_count + vm.purgeable_count) * qint64(page);
    return true;
#else
    return readMemInfo(totalBytes, availableBytes);
#endif
}

// --- EngineSettings ---------------------------------------------------------

EngineSettings::EngineSettings(Engine *engine, Settings *settings, QObject *parent)
    : EngineSettings(settings, [engine](const QString &path, const QJsonObject &body, Engine::Reply reply) {
          engine->post(path, body, std::move(reply));
      }, parent) {
    connect(engine, &Engine::started, this, [this] {
        m_settings->refreshAutoRamCache();
        applyAll();
    });
}

EngineSettings::EngineSettings(Settings *settings, Post post, QObject *parent)
    : QObject(parent), m_settings(settings), m_post(std::move(post)) {
    // Sliders change settings many times a second; the engine hears the last value.
    m_flushTimer.setSingleShot(true);
    m_flushTimer.setInterval(250);
    connect(&m_flushTimer, &QTimer::timeout, this, &EngineSettings::flush);
    connect(settings, &Settings::settingChanged, this, [this](const QString &key) {
        m_pending.insert(key);
        m_flushTimer.start();
    });
}

void EngineSettings::setConditions(const DeviceConditions &conditions) {
    const DeviceConditions before = m_conditions;
    m_conditions = conditions;
    if (networkAllowed(*m_settings, before) != networkAllowed(*m_settings, conditions) ||
        batteryPaused(*m_settings, before) != batteryPaused(*m_settings, conditions))
        m_pending.insert("networkPolicy");
    if (seedingAllowed(*m_settings, before) != seedingAllowed(*m_settings, conditions))
        m_pending.insert("seedingEnabled");
    if (!m_pending.isEmpty())
        m_flushTimer.start();
}

bool EngineSettings::networkAllowed(const Settings &s, const DeviceConditions &c) {
    // UiModels.kt transferAllowed().
    const QString policy = s.networkPolicy();
    if (!c.connected)
        return false;
    if (policy == "WIFI_ONLY")
        return c.wifi;
    if (policy == "UNMETERED")
        return !c.metered;
    return true;
}

bool EngineSettings::batteryPaused(const Settings &s, const DeviceConditions &c) {
    const int threshold = s.policy().setting("pauseOnLowBattery").value("batteryPercent").toInt(15);
    return s.pauseOnLowBattery() && c.batteryPercent >= 0 && c.batteryPercent <= threshold && !c.charging;
}

bool EngineSettings::seedingAllowed(const Settings &s, const DeviceConditions &c) {
    // LibrarySession.kt applySeedingSettings().
    return networkAllowed(s, c) && c.connected && (!s.seedWiFiOnly() || c.wifi) &&
           (!s.seedChargingOnly() || c.charging) && !c.playing;
}

static EngineRequest settingsRequest(const QString &field, const QJsonValue &value) {
    return {"/settings", QJsonObject{{field, value}}};
}

static EngineRequest seedingRequest(const Settings &s, const DeviceConditions &c) {
    // EngineClient.kt setSeeding().
    return {"/settings", QJsonObject{
        {"seedingEnabled", s.seedingEnabled()},
        {"seedingAllowed", EngineSettings::seedingAllowed(s, c)},
        {"seedRatioMilli", s.seedRatioMilli()},
        {"seedTimeMinutes", s.seedTimeMinutes()},
        {"uploadRateBytesSec", s.seedingEnabled() ? qint64(s.seedUploadKiB()) * 1024 : qint64(0)},
    }};
}

static EngineRequest requestForField(const QString &field, const Settings &s, const DeviceConditions &c) {
    const QJsonObject engine = s.policy().engine();
    if (field == "maxPeers")
        return settingsRequest(field, s.maxPeers());
    if (field == "downloadRateBytesSec")
        return settingsRequest(field, qint64(s.downloadRateKiB()) * 1024);
    if (field == "dialRatePerSec") {
        const bool fast = engine.value("fastPeerSearch").toBool();
        const int rate = engine.value(fast ? "fastPeerSearchDialRatePerSec" : "dialRatePerSec").toInt(30);
        return settingsRequest(field, std::clamp(rate, 10, 80));
    }
    if (field == "idleDropMinutes")
        return settingsRequest(field, s.idleDropMinutes());
    if (field == "ramCacheBytes")
        return settingsRequest(field, qint64(s.wantedRamCacheMiB()) * MiB);
    if (field == "seeding")
        return seedingRequest(s, c);
    if (field == "payloadTransfersAllowed")
        return settingsRequest(field, EngineSettings::networkAllowed(s, c) && !EngineSettings::batteryPaused(s, c));
    if (field == "bypass")
        return {"/bypass", QJsonObject{{"enabled", s.antiDpiEnabled()}}};
    // A hidden engine switch: its value is in the policy.
    return settingsRequest(field, engine.value(field));
}

QList<EngineRequest> EngineSettings::startRequests(const Settings &s, const DeviceConditions &c) {
    QList<EngineRequest> requests;
    for (const QJsonValue &field : s.policy().engine().value("startOrder").toArray())
        requests << requestForField(field.toString(), s, c);
    return requests;
}

QList<EngineRequest> EngineSettings::requestsFor(const QString &key, const Settings &s, const DeviceConditions &c) {
    QStringList fields;
    if (key == "maxPeers")
        fields = {"maxPeers"};
    else if (key == "downloadRateKiB")
        fields = {"downloadRateBytesSec"};
    else if (key == "idleDropMinutes")
        fields = {"idleDropMinutes"};
    else if (key == "ramCacheAuto" || key == "ramCacheMiB")
        fields = {"ramCacheBytes"};
    else if (key.startsWith("seed"))
        fields = {"seeding"};
    else if (key == "antiDpiEnabled")
        fields = {"bypass"};
    else if (key == "networkPolicy" || key == "pauseOnLowBattery")
        fields = {"payloadTransfersAllowed", "seeding"};
    QList<EngineRequest> requests;
    for (const QString &field : fields)
        requests << requestForField(field, s, c);
    return requests;
}

void EngineSettings::applyAll() {
    m_pending.clear();
    m_flushTimer.stop();
    send(startRequests(*m_settings, m_conditions));
}

void EngineSettings::flush() {
    QList<EngineRequest> requests;
    for (const QString &key : std::as_const(m_pending))
        for (const EngineRequest &r : requestsFor(key, *m_settings, m_conditions))
            if (!requests.contains(r))
                requests << r;
    m_pending.clear();
    send(requests);
}

void EngineSettings::send(const QList<EngineRequest> &requests) {
    for (const EngineRequest &r : requests) {
        Engine::Reply reply;
        if (r.body.contains("ramCacheBytes")) {
            // The engine may keep a larger size while a Direct Play torrent needs it.
            QPointer<Settings> settings = m_settings;
            reply = [settings](const QJsonObject &body, int status) {
                if (settings && status == 200 && body.contains("ramCacheBytes"))
                    settings->setAppliedRamCacheMiB(int(body.value("ramCacheBytes").toInteger() / MiB));
            };
        }
        m_post(r.path, r.body, reply);
    }
}
