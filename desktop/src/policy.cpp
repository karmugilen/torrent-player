#include "policy.h"

#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <algorithm>

const Policy &Policy::instance() {
    static const Policy policy = [] {
        Policy p = load(":/policy.json");
        if (!p.isValid())
            qWarning("policy: %s", qPrintable(p.error()));
        return p;
    }();
    return policy;
}

Policy Policy::load(const QString &path) {
    Policy p;
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) {
        p.m_error = QString("cannot read %1").arg(path);
        return p;
    }
    QJsonParseError parse;
    const QJsonDocument doc = QJsonDocument::fromJson(file.readAll(), &parse);
    if (parse.error != QJsonParseError::NoError || !doc.isObject()) {
        p.m_error = QString("%1: %2").arg(path, parse.errorString());
        return p;
    }
    p.m_root = doc.object();
    if (!p.m_root.value("settings").isObject())
        p.m_error = QString("%1: no settings section").arg(path);
    return p;
}

QStringList Policy::settingKeys(const QString &platform) const {
    QStringList keys;
    const QJsonObject settings = m_root.value("settings").toObject();
    for (auto it = settings.begin(); it != settings.end(); ++it) {
        if (it.key().startsWith('_') || !it.value().isObject())
            continue;
        const QJsonValue platforms = it.value().toObject().value("platforms");
        if (platforms.isArray() && !platforms.toArray().contains(platform))
            continue;
        keys << it.key();
    }
    keys.sort();
    return keys;
}

QJsonObject Policy::setting(const QString &key) const {
    return m_root.value("settings").toObject().value(key).toObject();
}

QVariant Policy::defaultValue(const QString &key) const {
    return setting(key).value("default").toVariant();
}

bool Policy::isVisible(const QString &key) const { return setting(key).value("visible").toBool(); }

QVariantList Policy::options(const QString &key) const {
    return setting(key).value("options").toArray().toVariantList();
}

QVariant Policy::normalize(const QString &key, const QVariant &value) const {
    const QJsonObject s = setting(key);
    const QJsonValue def = s.value("default");
    if (def.isBool())
        return value.typeId() == QMetaType::Bool ? value.toBool() : def.toBool();
    if (def.isString()) {
        const QString text = value.toString();
        const QVariantList choices = options(key);
        if (!value.isValid() || (!choices.isEmpty() && !choices.contains(text)))
            return def.toString();
        return text;
    }
    if (def.isDouble()) {
        bool ok = false;
        const int number = value.toInt(&ok);
        if (!value.isValid() || !ok)
            return def.toInt();
        if (s.contains("min") || s.contains("max"))
            return std::clamp(number, s.value("min").toInt(INT_MIN), s.value("max").toInt(INT_MAX));
        const QVariantList choices = options(key);
        if (!choices.isEmpty() && !choices.contains(number))
            return def.toInt();
        return number;
    }
    return value.isValid() ? value : def.toVariant();
}

QList<int> Policy::ramCacheStepsMiB() const {
    QList<int> steps;
    for (const QJsonValue &v : m_root.value("ramCache").toObject().value("stepsMiB").toArray())
        steps << v.toInt();
    return steps;
}

Policy::RamRule Policy::ramRule() const {
    RamRule rule;
    const QJsonObject a = m_root.value("ramCache").toObject().value("auto").toObject();
    if (a.isEmpty())
        return rule;
    rule.reserveTotalDivisor = std::max<qint64>(1, a.value("reserveTotalDivisor").toInteger(10));
    rule.budgetTotalDivisor = std::max<qint64>(1, a.value("budgetTotalDivisor").toInteger(16));
    rule.budgetAvailableDivisor = std::max<qint64>(1, a.value("budgetAvailableDivisor").toInteger(4));
    rule.minMiB = a.value("minMiB").toInt(64);
    QList<int> steps;
    for (const QJsonValue &v : a.value("autoStepsMiB").toArray())
        steps << v.toInt();
    if (!steps.isEmpty())
        rule.autoStepsMiB = steps;
    return rule;
}

int autoRamCacheMiB(qint64 totalBytes, qint64 availableBytes, int activeTransfers, const Policy::RamRule &rule) {
    // Android also reserves its low-memory threshold and treats low-RAM devices as
    // minimum; Linux has neither, so the reserve is a share of total memory.
    if (totalBytes <= 0)
        return rule.minMiB;
    const qint64 reserve = totalBytes / rule.reserveTotalDivisor;
    const qint64 available = std::max<qint64>(0, std::max<qint64>(0, availableBytes) - reserve);
    const qint64 budget = std::min(totalBytes / rule.budgetTotalDivisor, available / rule.budgetAvailableDivisor);
    const qint64 perTransferMiB = budget / std::max(1, activeTransfers) / (1024 * 1024);
    for (int step : rule.autoStepsMiB)
        if (step <= perTransferMiB)
            return step;
    return rule.minMiB;
}
