#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonObject>
#include <QTemporaryDir>
#include <QJsonDocument>
#include <QtTest>

#include "addsession.h"
#include "autostart.h"
#include "battery.h"
#include "connections.h"
#include "engine.h"
#include "format.h"
#include "library.h"
#include "links.h"
#include "policy.h"
#include "settings.h"
#include "playback.h"
#include "subtitles.h"
#include "theme.h"
#include "translator.h"

#include <QSignalSpy>

static QJsonObject file(int index, const QString &path, double length, const QString &type = {}) {
    return {{"index", index}, {"name", path.section('/', -1)}, {"path", path}, {"length", length}, {"type", type}};
}

class TorrentPlayerTests : public QObject {
    Q_OBJECT

private slots:
    void magnetHashes() {
        QCOMPARE(Links::magnetInfoHash("magnet:?xt=urn:btih:08ADA5A7A6183AAE1E09D831DF6748D566095A10&dn=Sintel"),
                 QString("08ada5a7a6183aae1e09d831df6748d566095a10"));
        // Base32 form of the same hash.
        QCOMPARE(Links::magnetInfoHash("magnet:?xt=urn:btih:BCW2LJ5GDA5K4HQJ3AY56Z2I2VTASWQQ"),
                 QString("08ada5a7a6183aae1e09d831df6748d566095a10"));
        QCOMPARE(Links::magnetInfoHash("magnet:?xt=urn:btmh:1220" + QString(64, 'a')), QString(64, 'a'));
        QVERIFY(Links::magnetInfoHash("magnet:?dn=nothing").isEmpty());
        QVERIFY(Links::magnetInfoHash("https://example.com").isEmpty());
    }

    void classifiesInput() {
        QTemporaryDir dir;
        const QString path = dir.filePath("a.torrent");
        QFile f(path);
        QVERIFY(f.open(QIODevice::WriteOnly));
        f.write("d4:infod");
        f.close();
        QCOMPARE(Links::classify("magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10"), Links::Kind::Magnet);
        QCOMPARE(Links::classify("magnet:?dn=x"), Links::Kind::Invalid);
        QCOMPARE(Links::classify("https://example.com/a.torrent"), Links::Kind::TorrentUrl);
        QCOMPARE(Links::classify(path), Links::Kind::TorrentFile);
        QCOMPARE(Links::classify("file://" + path), Links::Kind::TorrentFile);
        QCOMPARE(Links::classify("hello"), Links::Kind::Invalid);
        QCOMPARE(Links::classify(""), Links::Kind::Invalid);
    }

    void fileKinds() {
        QVERIFY(Links::isVideo("a.MKV"));
        QVERIFY(Links::isVideo("a.bin", "video/mp4"));
        QVERIFY(!Links::isVideo("a.srt"));
        QVERIFY(Links::isSubtitle("a.en.srt"));
        QVERIFY(Links::subtitleMatches("Movie.mkv", "Movie.en.srt"));
        QVERIFY(!Links::subtitleMatches("Movie.mkv", "Other.en.srt"));
    }

    void defaultSelectionPicksVideosAndSubtitles() {
        const QJsonArray files{file(0, "S/Sintel.de.srt", 10), file(1, "S/Sintel.mp4", 100, "video/mp4"),
                               file(2, "S/poster.jpg", 5), file(3, "S/readme.txt", 1)};
        QCOMPARE(AddSession::defaultSelection(files), (QList<int>{0, 1}));
        const QJsonArray noVideo{file(0, "a.iso", 10), file(1, "b.txt", 1)};
        QCOMPARE(AddSession::defaultSelection(noVideo), (QList<int>{0, 1}));
    }

    void formats() {
        QLocale::setDefault(QLocale::c());
        QCOMPARE(Format::clock(0), QString("0:00"));
        QCOMPARE(Format::clock(65000), QString("1:05"));
        QCOMPARE(Format::clock(3723000), QString("1:02:03"));
        QCOMPARE(Format::eta(0), QString());
        QCOMPARE(Format::eta(40), QString("40 s"));
        QCOMPARE(Format::eta(300), QString("5 min"));
        QCOMPARE(Format::eta(3 * 3600 + 240), QString("3 h 4 min"));
        QCOMPARE(Format::percent(0.42), QString("42%"));
        QCOMPARE(Format::percent(0.9965), QString("99.6%"));
        QCOMPARE(Format::percent(1), QString("100%"));
        QVERIFY(Format::bytes(129241752).contains("MB"));
    }

    void parsesSubRip() {
        const QByteArray srt = "1\r\n00:00:01,000 --> 00:00:02,500\r\n<i>Hello</i>\r\nthere\r\n\r\n"
                               "2\r\n00:00:03,000 --> 00:00:04,000\r\nSecond\r\n";
        Subtitles s;
        s.setCues(Subtitles::parse(srt), "x");
        QCOMPARE(s.cues().size(), 2);
        QCOMPARE(s.textAt(500), QString());
        QCOMPARE(s.textAt(1500), QString("Hello\nthere"));
        QCOMPARE(s.textAt(2700), QString());
        QCOMPARE(s.textAt(3500), QString("Second"));
    }

    void parsesWebVtt() {
        const QByteArray vtt = "WEBVTT\n\n00:01.000 --> 00:02.000 align:start\nHi\n\n01:00:00.000 --> 01:00:01.000\nLate\n";
        const auto cues = Subtitles::parse(vtt);
        QCOMPARE(cues.size(), 2);
        QCOMPARE(cues[0].start, 1000);
        QCOMPARE(cues[1].start, 3600000);
    }

    void libraryEntryRoundTrips() {
        Library::Entry e;
        e.key = "abc";
        e.magnet = "magnet:?xt=urn:btih:abc";
        e.name = "Name";
        e.selected = {1, 3};
        e.files = QJsonArray{file(1, "a/b.mkv", 5)};
        e.paused = true;
        e.exported = {3};
        e.positions = {{"1", 42000.0}};
        e.lastFile = 1;
        const Library::Entry back = Library::fromJson(Library::toJson(e));
        QCOMPARE(back.key, e.key);
        QCOMPARE(back.selected, e.selected);
        QCOMPARE(back.paused, true);
        QCOMPARE(back.exported, e.exported);
        QCOMPARE(back.positions.value("1").toDouble(), 42000.0);
        QCOMPARE(back.lastFile, 1);
    }

    void libraryPersistsAndKeepsExportsInsideSaveFolder() {
        QTemporaryDir dir;
        Engine engine(dir.filePath("engine"));
        {
            Library library(&engine, dir.filePath("data"), dir.filePath("save"));
            library.adopt("id", "hash1", "magnet:?xt=urn:btih:hash1", "Show",
                          QJsonArray{file(0, "Show/../../evil.mkv", 10), file(1, "Show/ep.srt", 1)}, {0}, "d4:infoe");
            QCOMPARE(library.count(), 1);
            const auto *e = library.entry("hash1");
            QVERIFY(e);
            const QString path = library.exportPath(*e, 0);
            QVERIFY2(path.startsWith(dir.filePath("save") + "/"), qPrintable(path));
            QVERIFY(!path.contains(".."));
        }
        Library reloaded(&engine, dir.filePath("data"), dir.filePath("save"));
        QCOMPARE(reloaded.count(), 1);
        QCOMPARE(reloaded.entry("hash1")->name, QString("Show"));
        QVERIFY(QFile::exists(dir.filePath("data/torrents/hash1.torrent")));
        reloaded.remove("hash1", false);
        QCOMPARE(reloaded.count(), 0);
        QVERIFY(!QFile::exists(dir.filePath("data/torrents/hash1.torrent")));
    }

    void displayStates() {
        Library::Entry e;
        QCOMPARE(Library::displayState(e), QString("starting"));
        e.id = "x";
        QCOMPARE(Library::displayState(e), QString("fetching"));
        e.ready = true;
        QCOMPARE(Library::displayState(e), QString("searching"));
        e.peers = 3;
        QCOMPARE(Library::displayState(e), QString("waiting"));
        e.downloadSpeed = 10;
        QCOMPARE(Library::displayState(e), QString("downloading"));
        e.paused = true;
        QCOMPARE(Library::displayState(e), QString("paused"));
        e.error = "boom";
        QCOMPARE(Library::displayState(e), QString("error"));
    }

    void themeFallsBack() {
        QTemporaryDir dir;
        Theme theme(dir.path());
        QCOMPARE(theme.accent(), QString("#FFD60A"));
        QCOMPARE(theme.accentForeground(), QString("black"));
    }

    void themeReadsAccent() {
        QTemporaryDir dir;
        QDir(dir.path()).mkpath("theme");
        QFile colors(dir.path() + "/theme/colors.toml");
        QVERIFY(colors.open(QIODevice::WriteOnly));
        colors.write("accent = \"#112233\"\n");
        colors.close();
        Theme theme(dir.path());
        QCOMPARE(theme.accent(), QString("#112233"));
        QCOMPARE(theme.accentForeground(), QString("white"));
    }

    void translatorFallsBackToEnglish() {
        QTemporaryDir dir;
        Translator translator(dir.path() + "/language", dir.path());
        QCOMPARE(translator.language(), QString("en"));
        QVERIFY(translator.translate("Main", "Save").isNull());
    }

    void englishPlurals() {
        QTemporaryDir dir;
        Translator translator(dir.path() + "/language", ":/nothing");
        // The shipped catalog is compiled into the app; check its JSON directly.
        QFile catalog(QStringLiteral(SRCDIR "/../src/i18n/en.json"));
        QVERIFY(catalog.open(QIODevice::ReadOnly));
        const auto object = QJsonDocument::fromJson(catalog.readAll()).object();
        QCOMPARE(object.value("%n peers").toObject().value("one").toString(), QString("%n peer"));
    }

    // --- shared/policy.json and settings ---

    void policyLoads() {
        const Policy &compiled = Policy::instance();
        QVERIFY2(compiled.isValid(), qPrintable(compiled.error()));
        const Policy file = Policy::load(SRCDIR "/../../shared/policy.json");
        QVERIFY2(file.isValid(), qPrintable(file.error()));
        QCOMPARE(compiled.root(), file.root());
        QVERIFY(!Policy::load("/nonexistent/policy.json").isValid());
        QCOMPARE(compiled.ramCacheStepsMiB(),
                 QList<int>({16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024}));
        // Android-only settings stay out of the desktop.
        QVERIFY(!compiled.settingKeys().contains("playbackMethod"));
        QVERIFY(compiled.settingKeys("android").contains("playbackMethod"));
        // builtInPlayer is shared; desktop uses it for built-in vs mpv window.
        QVERIFY(compiled.settingKeys().contains("builtInPlayer"));
        QVERIFY(compiled.settingKeys("android").contains("builtInPlayer"));
        QCOMPARE(compiled.defaultValue("builtInPlayer").toBool(), true);
        // Every setting says whether Android shows it and has a default.
        for (const QString &key : compiled.settingKeys("android")) {
            QVERIFY2(compiled.setting(key).value("visible").isBool(), qPrintable(key));
            QVERIFY2(compiled.defaultValue(key).isValid(), qPrintable(key));
        }
        // The fixed playback mode and hidden engine switches Android uses.
        const QJsonObject play = compiled.play(), engine = compiled.engine();
        QVERIFY(play.value("boost").toBool() && play.value("fileOnly").toBool() && play.value("adaptiveBuffer").toBool());
        QVERIFY(!play.value("burstAwareBuffer").toBool() && !play.value("preloadStart").toBool());
        QVERIFY(play.value("playNext").toBool());
        QVERIFY(engine.value("ghostProMode").toBool() && !engine.value("ghostMode").toBool());
        QVERIFY(engine.value("claudeSeekMode").toBool() && !engine.value("deadlineSeek").toBool());
        QCOMPARE(engine.value("dialRatePerSec").toInt(), 30);
    }

    void settingsDefaultsMatchPolicy() {
        QTemporaryDir dir;
        Settings settings(dir.filePath("settings.json"));
        const Policy &policy = Policy::instance();
        QVERIFY(!settings.keys().isEmpty());
        for (const QString &key : settings.keys())
            QCOMPARE(settings.value(key), policy.defaultValue(key));
        // Android's UiState / LibrarySession defaults.
        QCOMPARE(settings.directPlayDefault(), false);
        QCOMPARE(settings.idleDropMinutes(), 15);
        QCOMPARE(settings.builtInPlayer(), true);
        QCOMPARE(settings.networkPolicy(), QString("ANY"));
        QCOMPARE(settings.downloadRateKiB(), 0);
        QCOMPARE(settings.maxPeers(), 55);
        QCOMPARE(settings.antiDpiEnabled(), false);
        QCOMPARE(settings.seedingEnabled(), true);
        QCOMPARE(settings.seedRatioMilli(), 1000);
        QCOMPARE(settings.seedTimeMinutes(), 60);
        QCOMPARE(settings.seedUploadKiB(), 256);
        QCOMPARE(settings.seedWiFiOnly(), true);
        QCOMPARE(settings.seedChargingOnly(), false);
        QCOMPARE(settings.pauseOnLowBattery(), false);
        QCOMPARE(settings.ramCacheAuto(), true);
        QCOMPARE(settings.value("ramCacheMiB").toInt(), 64);
        QVERIFY(settings.ramCacheMiB() == 64 || settings.ramCacheMiB() == 128);
        QCOMPARE(settings.ramCacheBytes(), qint64(settings.ramCacheMiB()) * 1024 * 1024);
    }

    void autoRamCacheMatchesAndroid() {
        // RamCachePolicyTest.kt cases with a low-memory threshold below a tenth of RAM,
        // so Android reserves the same; results follow RamCachePolicy.kt's 128 MiB cap.
        const qint64 gib = 1024LL * 1024 * 1024;
        QCOMPARE(autoRamCacheMiB(8 * gib, 4 * gib, 1), 128);
        QCOMPARE(autoRamCacheMiB(8 * gib, 4 * gib, 2), 128);
        QCOMPARE(autoRamCacheMiB(8 * gib, 4 * gib, 4), 128);
        QCOMPARE(autoRamCacheMiB(8 * gib, 4 * gib, 5), 64);
        QCOMPARE(autoRamCacheMiB(8 * gib, gib + gib / 2, 1), 128);
        QCOMPARE(autoRamCacheMiB(8 * gib, gib / 2, 1), 64);
        QCOMPARE(autoRamCacheMiB(16 * gib, 10 * gib, 0), 128);
        QCOMPARE(autoRamCacheMiB(0, 0, -1), 64);
        QCOMPARE(autoRamCacheMiB(8 * gib, -1, 1), 64);
        const Policy::RamRule rule = Policy::instance().ramRule();
        QCOMPARE(rule.reserveTotalDivisor, 10);
        QCOMPARE(rule.budgetTotalDivisor, 16);
        QCOMPARE(rule.budgetAvailableDivisor, 4);
        QCOMPARE(rule.autoStepsMiB, QList<int>({128, 64}));
        QCOMPARE(rule.minMiB, 64);
    }

    void readsMemInfo() {
        QTemporaryDir dir;
        QFile f(dir.filePath("meminfo"));
        QVERIFY(f.open(QIODevice::WriteOnly));
        f.write("MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:    8000000 kB\n");
        f.close();
        qint64 total = 0, available = 0;
        QVERIFY(readMemInfo(total, available, f.fileName()));
        QCOMPARE(total, 16000000LL * 1024);
        QCOMPARE(available, 8000000LL * 1024);
        QVERIFY(!readMemInfo(total, available, dir.filePath("missing")));
    }

    void settingsSaveAndReload() {
        QTemporaryDir dir;
        const QString path = dir.filePath("sub/settings.json");
        {
            Settings settings(path);
            QSignalSpy spy(&settings, &Settings::settingChanged);
            settings.setMaxPeers(120);
            settings.setSeedingEnabled(false);
            settings.setNetworkPolicy("WIFI_ONLY");
            settings.setDownloadRateKiB(4096);
            settings.setRamCacheMiB(256);
            QCOMPARE(spy.count(), 5);
            settings.setMaxPeers(120);
            QCOMPARE(spy.count(), 5);
            QVERIFY(!settings.ramCacheAuto());
            QCOMPARE(settings.ramCacheMiB(), 256);
            QVERIFY(settings.save());
        }
        {
            Settings settings(path);
            QCOMPARE(settings.maxPeers(), 120);
            QCOMPARE(settings.seedingEnabled(), false);
            QCOMPARE(settings.networkPolicy(), QString("WIFI_ONLY"));
            QCOMPARE(settings.downloadRateKiB(), 4096);
            QCOMPARE(settings.ramCacheAuto(), false);
            QCOMPARE(settings.ramCacheBytes(), 256LL * 1024 * 1024);
            // Out-of-range values are clamped or fall back, as Android's coerceIn does.
            settings.setMaxPeers(9999);
            QCOMPARE(settings.maxPeers(), 500);
            settings.setDownloadRateKiB(3000);
            QCOMPARE(settings.downloadRateKiB(), 0);
            settings.setNetworkPolicy("BOGUS");
            QCOMPARE(settings.networkPolicy(), QString("ANY"));
            settings.reset();
            QCOMPARE(settings.maxPeers(), 55);
            QCOMPARE(settings.ramCacheAuto(), true);
        }
        {
            Settings settings(path);  // reset() saved
            QCOMPARE(settings.maxPeers(), 55);
            QCOMPARE(settings.seedingEnabled(), true);
        }
        // A broken or hand-edited file still loads.
        QFile f(path);
        QVERIFY(f.open(QIODevice::WriteOnly));
        f.write(R"({"maxPeers": 2, "seedWiFiOnly": "yes", "ramCacheMiB": 96})");
        f.close();
        Settings settings(path);
        QCOMPARE(settings.maxPeers(), 8);
        QCOMPARE(settings.seedWiFiOnly(), true);
        QCOMPARE(settings.ramCacheAuto(), false);  // a chosen size without the Auto flag
        QCOMPARE(settings.ramCacheMiB(), 96);
    }

    void engineGetsAndroidStartRequests() {
        QTemporaryDir dir;
        Settings settings(dir.filePath("settings.json"));
        settings.setRamCacheMiB(128);
        DeviceConditions device;
        const QList<EngineRequest> r = EngineSettings::startRequests(settings, device);
        auto at = [&](int i) { return r.value(i).body; };
        QCOMPARE(r.size(), 16);
        QCOMPARE(at(0), QJsonObject({{"maxPeers", 55}}));
        QCOMPARE(at(1), QJsonObject({{"downloadRateBytesSec", 0}}));
        QCOMPARE(at(2), QJsonObject({{"dialRatePerSec", 30}}));
        QCOMPARE(at(3), QJsonObject({{"peerExploration", false}}));
        QCOMPARE(at(4), QJsonObject({{"ghostMode", false}}));
        QCOMPARE(at(5), QJsonObject({{"ghostProMode", true}}));
        QCOMPARE(at(6), QJsonObject({{"claudeSeekMode", true}}));
        QCOMPARE(at(7), QJsonObject({{"hedgeRequests", true}}));
        QCOMPARE(at(8), QJsonObject({{"slowPeerYield", true}}));
        QCOMPARE(at(9), QJsonObject({{"dropChokers", true}}));
        QCOMPARE(at(10), QJsonObject({{"deadlineSeek", false}}));
        QCOMPARE(at(11), QJsonObject({{"idleDropMinutes", 15}}));
        QCOMPARE(at(12), QJsonObject({{"ramCacheBytes", 128LL * 1024 * 1024}}));
        QCOMPARE(at(13), QJsonObject({{"seedingEnabled", true}, {"seedingAllowed", true}, {"seedRatioMilli", 1000},
                                      {"seedTimeMinutes", 60}, {"uploadRateBytesSec", 256 * 1024}}));
        QCOMPARE(at(14), QJsonObject({{"payloadTransfersAllowed", true}}));
        QCOMPARE(r[15].path, QString("/bypass"));
        QCOMPARE(at(15), QJsonObject({{"enabled", false}}));
        for (int i = 0; i < 15; ++i)
            QCOMPARE(r[i].path, QString("/settings"));

        // Seeding pauses while playing, off Wi-Fi when asked, and sends no upload rate when off.
        device.playing = true;
        QVERIFY(!EngineSettings::seedingAllowed(settings, device));
        device.playing = false;
        device.wifi = false;
        QVERIFY(!EngineSettings::seedingAllowed(settings, device));
        settings.setNetworkPolicy("WIFI_ONLY");
        QVERIFY(!EngineSettings::networkAllowed(settings, device));
        settings.setPauseOnLowBattery(true);
        device = {};
        device.charging = false;
        device.batteryPercent = 10;
        QVERIFY(EngineSettings::batteryPaused(settings, device));
        settings.setSeedingEnabled(false);
        const QList<EngineRequest> seeding = EngineSettings::requestsFor("seedingEnabled", settings, device);
        QCOMPARE(seeding.size(), 1);
        QCOMPARE(seeding[0].body.value("uploadRateBytesSec").toInteger(), 0);
    }

    void engineHearsSettingChanges() {
        QTemporaryDir dir;
        Settings settings(dir.filePath("settings.json"));
        QList<EngineRequest> sent;
        EngineSettings engineSettings(&settings, [&](const QString &path, const QJsonObject &body, Engine::Reply reply) {
            sent << EngineRequest{path, body};
            if (reply)
                reply(QJsonObject{{"ramCacheBytes", 192LL * 1024 * 1024}}, 200);
        });
        engineSettings.applyAll();
        QCOMPARE(sent.size(), 16);
        QCOMPARE(settings.ramCacheMiB(), 192);  // what the engine applied
        sent.clear();
        // A dragged slider: only the last value goes out.
        for (int n = 60; n <= 80; ++n)
            settings.setMaxPeers(n);
        settings.setAntiDpiEnabled(true);
        QTRY_COMPARE(sent.size(), 2);
        QVERIFY(sent.contains(EngineRequest{"/settings", {{"maxPeers", 80}}}));
        QVERIFY(sent.contains(EngineRequest{"/bypass", {{"enabled", true}}}));
        sent.clear();
        settings.setDirectPlayDefault(true);  // app-only setting
        QTest::qWait(400);
        QVERIFY(sent.isEmpty());
    }

    void parsesConnectionsSnapshot() {
        const QJsonObject root = QJsonDocument::fromJson(R"({
            "id":"abc","paused":false,
            "peers":[
                {"address":"8.8.8.8:1234","transport":"TCP","client":"Local peer",
                 "downloaded":4096,"uploaded":1024,"country":"US"},
                {"address":"203.0.113.1:6881","transport":"uTP","client":"qBittorrent",
                 "downloaded":100,"uploaded":50,"country":"DE","source":"C",
                 "bittorrentVersion":"v2","downloadRate":2048.5,"peerCompletion":0.42},
                {"address":"2001:db8::1:51413","transport":"TCP","client":"",
                 "downloaded":0,"uploaded":0}
            ]
        })").object();
        QVERIFY(!Connections::parsePaused(root));
        const auto peers = Connections::parsePeers(root);
        QCOMPARE(peers.size(), 3);
        QCOMPARE(peers[0].address, QString("8.8.8.8:1234"));
        QCOMPARE(peers[0].country, QString("US"));
        QCOMPARE(peers[0].downloaded, 4096);
        QCOMPARE(peers[0].uploaded, 1024);
        QCOMPARE(peers[0].downloadRate, -1.0);
        QCOMPARE(peers[0].progress, -1.0);
        QVERIFY(!peers[0].holePunched);
        QCOMPARE(peers[1].transport, QString("uTP"));
        QVERIFY(peers[1].holePunched);
        QCOMPARE(peers[1].bittorrentVersion, QString("v2"));
        QCOMPARE(peers[1].downloadRate, 2048.5);
        QCOMPARE(peers[1].progress, 0.42);
        QCOMPARE(peers[2].client, QString());

        const QHash<QString, QVariantMap> labels{
            {"US", {{"code", "US"}, {"name", "United States"}, {"longitude", -100.0}, {"latitude", 40.0}}},
            {"DE", {{"code", "DE"}, {"name", "Germany"}, {"longitude", 10.0}, {"latitude", 51.0}}},
        };
        const QVariantList countries = Connections::countryCounts(peers, labels);
        QCOMPARE(countries.size(), 2);
        // Equal counts sort by country code.
        QCOMPARE(countries[0].toMap().value("code").toString(), QString("DE"));
        QCOMPARE(countries[0].toMap().value("count").toInt(), 1);
        QCOMPARE(countries[0].toMap().value("name").toString(), QString("Germany"));
        QCOMPARE(countries[1].toMap().value("code").toString(), QString("US"));
        QCOMPARE(countries[1].toMap().value("name").toString(), QString("United States"));

        const QJsonObject paused = QJsonDocument::fromJson(R"({"id":"abc","paused":true,"peers":[]})").object();
        QVERIFY(Connections::parsePaused(paused));
        QVERIFY(Connections::parsePeers(paused).isEmpty());
    }

    void loadsWorldMapLabels() {
        QFile file(QStringLiteral(":/world-map.json"));
        QVERIFY2(file.open(QIODevice::ReadOnly), "world-map.json missing from resources");
        const auto labels = Connections::loadMapLabels(file.readAll());
        QVERIFY(labels.contains("US"));
        QCOMPARE(labels.value("DE").value("name").toString(), QString("Germany"));
        QVERIFY(labels.value("FJ").value("longitude").toDouble() != 0);
    }

    void prefetchOffsetFollowsBitrate() {
        const qint64 mib = 1 << 20;
        // Halfway through a 100 MiB file starts 2 MiB before the 50 MiB estimate.
        QCOMPARE(PlaybackControl::prefetchOffset(100 * mib, 60000, 120000), 48 * mib);
        // Near the start never goes below zero; past the end stays inside the file.
        QCOMPARE(PlaybackControl::prefetchOffset(100 * mib, 1000, 120000), qint64(0));
        QCOMPARE(PlaybackControl::prefetchOffset(100 * mib, 999999, 120000), 98 * mib);
        QCOMPARE(PlaybackControl::prefetchOffset(0, 1000, 120000), qint64(0));
        QCOMPARE(PlaybackControl::prefetchOffset(100 * mib, 1000, 0), qint64(0));
        QCOMPARE(PlaybackControl::PrefetchLength, 10 * mib);
    }

    void fileOffsetSumsEarlierFiles() {
        const QJsonArray files{file(2, "c.srt", 50), file(0, "a.nfo", 100), file(1, "b.mkv", 1000)};
        QCOMPARE(PlaybackControl::fileOffset(files, 0), qint64(0));
        QCOMPARE(PlaybackControl::fileOffset(files, 1), qint64(100));
        QCOMPARE(PlaybackControl::fileOffset(files, 2), qint64(1100));
    }

    void piecesMapToFileFractions() {
        // 10 pieces of 100 bytes; the file is bytes 150..950 (pieces 1..9).
        const QJsonObject pieces{
            {"pieceLength", 100},
            {"buckets", QJsonArray{
                QJsonObject{{"start", 0}, {"end", 1}, {"total", 2}, {"verified", 2}},
                QJsonObject{{"start", 2}, {"end", 3}, {"total", 2}, {"verified", 2}},
                QJsonObject{{"start", 4}, {"end", 5}, {"total", 2}, {"verified", 1}},
                QJsonObject{{"start", 6}, {"end", 7}, {"total", 2}, {"verified", 0}},
                QJsonObject{{"start", 8}, {"end", 9}, {"total", 2}, {"verified", 2}}}}};
        const auto spans = PlaybackControl::spansFromPieces(pieces, 150, 800);
        QCOMPARE(spans.size(), 5);
        QCOMPARE(spans[0].from, 0.0);
        QCOMPARE(spans[0].to, 50.0 / 800);
        QCOMPARE(spans[2].done, 0.5);
        QCOMPARE(spans[4].to, 1.0);

        const auto ready = PlaybackControl::mergeReady(spans);
        QCOMPARE(ready.size(), 2);
        QCOMPARE(ready[0].first, 0.0);
        QCOMPARE(ready[0].second, 250.0 / 800);
        QCOMPARE(ready[1].first, 650.0 / 800);
        QCOMPARE(ready[1].second, 1.0);

        QCOMPARE(PlaybackControl::readyAt(spans, 0.1), QVariant(true));
        QCOMPARE(PlaybackControl::readyAt(spans, 0.5), QVariant(false));
        QCOMPARE(PlaybackControl::readyAt(spans, 0.95), QVariant(true));
        QVERIFY(!PlaybackControl::readyAt({}, 0.5).isValid());
        // No piece length: nothing known.
        QVERIFY(PlaybackControl::spansFromPieces(QJsonObject{{"buckets", QJsonArray{}}}, 0, 10).isEmpty());
    }

    void scrubSeeksLiveOnlyWhereReady() {
        // Downloaded spot, last seek long ago: seek now.
        auto step = PlaybackControl::scrubDecision(90000, 60000, 60000, 65000, true, 1000, false);
        QCOMPARE(step.value("action").toString(), QString("seek"));
        QCOMPARE(step.value("label").toString(), QString("1:30  [+0:30]"));
        QCOMPARE(step.value("note").toString(), QString());
        // Within 250 ms of the last live seek, or while mpv is still seeking: wait.
        QCOMPARE(PlaybackControl::scrubDecision(90000, 60000, 60000, 65000, true, 100, false).value("action").toString(), QString("wait"));
        QCOMPARE(PlaybackControl::scrubDecision(90000, 60000, 60000, 65000, true, 1000, true).value("action").toString(), QString("wait"));
        // In mpv's buffer counts as ready even when not downloaded.
        QCOMPARE(PlaybackControl::scrubDecision(62000, 60000, 60000, 65000, false, 1000, false).value("action").toString(), QString("seek"));
        // Not downloaded and not buffered: prefetch, seek on release, say it will buffer.
        step = PlaybackControl::scrubDecision(30000, 3725000, 3725000, 3730000, false, 1000, false);
        QCOMPARE(step.value("action").toString(), QString("prefetch"));
        QCOMPARE(step.value("note").toString(), QString("will buffer"));
        QCOMPARE(step.value("label").toString(), QString("0:30  [-1:01:35]"));
        // Nothing known yet: prefetch, but no "will buffer".
        step = PlaybackControl::scrubDecision(30000, 60000, 60000, 65000, QVariant(), 1000, false);
        QCOMPARE(step.value("action").toString(), QString("prefetch"));
        QCOMPARE(step.value("note").toString(), QString());
    }

    void autostartWritesDesktopFile() {
        QTemporaryDir dir;
        Autostart autostart(dir.path());
        QVERIFY(!autostart.enabled());
        autostart.setEnabled(true);
        QVERIFY(autostart.enabled());
        QFile file(autostart.desktopFilePath());
        QVERIFY(file.open(QIODevice::ReadOnly));
        const QString body = QString::fromUtf8(file.readAll());
        QVERIFY(body.contains("Exec=torrentplayer --background"));
        QVERIFY(body.contains("X-GNOME-Autostart-enabled=true"));
        QCOMPARE(Autostart::desktopFileContents().contains("torrentplayer --background"), true);
        autostart.setEnabled(false);
        QVERIFY(!autostart.enabled());
        QVERIFY(!QFile::exists(autostart.desktopFilePath()));
    }

    void batteryThresholdPausesWhenLow() {
        QVERIFY(DeviceStatus::shouldPauseTransfers(true, 10, false, 15));
        QVERIFY(DeviceStatus::shouldPauseTransfers(true, 15, false, 15));
        QVERIFY(!DeviceStatus::shouldPauseTransfers(true, 16, false, 15));
        QVERIFY(!DeviceStatus::shouldPauseTransfers(true, 10, true, 15));
        QVERIFY(!DeviceStatus::shouldPauseTransfers(false, 5, false, 15));
        // No battery (percent < 0): never pause.
        QVERIFY(!DeviceStatus::shouldPauseTransfers(true, -1, false, 15));
        QVERIFY(!DeviceStatus::shouldPauseTransfers(true, -1, true, 15));

        QTemporaryDir dir;
        Settings settings(dir.filePath("settings.json"));
        settings.setPauseOnLowBattery(true);
        DeviceConditions low = DeviceStatus::conditionsFrom(true, true, false, false, 10, false);
        QVERIFY(EngineSettings::batteryPaused(settings, low));
        DeviceConditions charging = DeviceStatus::conditionsFrom(true, true, false, true, 10, false);
        QVERIFY(!EngineSettings::batteryPaused(settings, charging));
        DeviceConditions none = DeviceStatus::conditionsFrom(true, true, false, true, -1, false);
        QVERIFY(!EngineSettings::batteryPaused(settings, none));
    }

    void orphanBoundaryKeysSkipLibraryTorrents() {
        const QStringList onDisk{"aaa", "bbb", "ccc"};
        const QStringList library{"bbb", "ddd"};
        QCOMPARE(orphanBoundaryKeys(onDisk, library), QStringList({"aaa", "ccc"}));
        QVERIFY(orphanBoundaryKeys(onDisk, onDisk).isEmpty());
        QVERIFY(orphanBoundaryKeys({}, library).isEmpty());
        QCOMPARE(orphanBoundaryKeys({"xyz"}, {}), QStringList({"xyz"}));
    }
};

QTEST_MAIN(TorrentPlayerTests)
#include "torrentplayer_tests.moc"
