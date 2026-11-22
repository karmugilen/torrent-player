#pragma once

#include <QJsonObject>
#include <QList>
#include <QString>
#include <QStringList>
#include <QVariant>

// The tuning both apps share: ../../shared/policy.json, compiled in as :/policy.json.
// Holds user setting defaults and ranges, the RAM cache rule, the /play flags and the
// hidden engine switches (see shared/README.md).
class Policy {
public:
    struct RamRule {
        qint64 reserveTotalDivisor = 10;
        qint64 budgetTotalDivisor = 16;
        qint64 budgetAvailableDivisor = 4;
        QList<int> autoStepsMiB{128, 64};
        int minMiB = 64;
    };

    // The compiled-in policy, loaded once.
    static const Policy &instance();
    // Loads a policy file; check isValid() (error() says why it is not).
    static Policy load(const QString &path);

    bool isValid() const { return m_error.isEmpty() && !m_root.isEmpty(); }
    QString error() const { return m_error; }
    QJsonObject root() const { return m_root; }

    // Sorted keys of the user settings this platform ("desktop" or "android") has.
    QStringList settingKeys(const QString &platform = "desktop") const;
    QJsonObject setting(const QString &key) const;
    QVariant defaultValue(const QString &key) const;
    bool isVisible(const QString &key) const;
    QVariantList options(const QString &key) const;
    // A saved value made valid: the default type, clamped to min/max, or the default when
    // only options are given and it is not one of them.
    QVariant normalize(const QString &key, const QVariant &value) const;

    QList<int> ramCacheStepsMiB() const;
    RamRule ramRule() const;
    QJsonObject play() const { return m_root.value("play").toObject(); }
    QJsonObject engine() const { return m_root.value("engine").toObject(); }

private:
    QJsonObject m_root;
    QString m_error;
};

// Memory per torrent when "Auto" is on, in MiB (RamCachePolicy.kt): keeps a tenth of RAM
// for the system, then gives each active transfer its share of min(total/16, free/4),
// rounded down to 128 or 64 MiB. Pure: the caller reads /proc/meminfo.
int autoRamCacheMiB(qint64 totalBytes, qint64 availableBytes, int activeTransfers,
                    const Policy::RamRule &rule = Policy::instance().ramRule());
