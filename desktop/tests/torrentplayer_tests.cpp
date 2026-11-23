#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonObject>
#include <QTemporaryDir>
#include <QJsonDocument>
#include <QtTest>

#include "addsession.h"
#include "connections.h"
#include "engine.h"
#include "format.h"
#include "library.h"
#include "links.h"
#include "policy.h"
#include "settings.h"
#include "playback.h"
#include "piecegrid.h"
#include "playerstats.h"
#include "series.h"
#include "subtitles.h"
#include "theme.h"
#include "translator.h"

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

    // ---- Direct Play and play start, as Android's LibrarySession and PlaybackLaunch.kt.

    void resumeByteHintFollowsTheTimeFraction() {
        // Unknown length or duration: no hint.
        QCOMPARE(Library::resumeByteHint(0, 1000, 10000), qint64(-1));
        QCOMPARE(Library::resumeByteHint(1000, 1000, 0), qint64(-1));
        QCOMPARE(Library::resumeByteHint(1000, -1, 10000), qint64(-1));
        QCOMPARE(Library::resumeByteHint(1000, 0, 10000), qint64(0));
        QCOMPARE(Library::resumeByteHint(1000, 2500, 10000), qint64(250));
        // Past the end clamps to the last byte.
        QCOMPARE(Library::resumeByteHint(1000, 20000, 10000), qint64(999));
        QCOMPARE(Library::resumeByteHint(4000000000LL, 5000, 10000), qint64(2000000000LL));
    }

    void savedPositionResetsNearTheEnd() {
        QCOMPARE(Library::savedPosition(97001, 100000), qint64(0));   // within 3 s of the end
        QCOMPARE(Library::savedPosition(97000, 100000), qint64(0));
        QCOMPARE(Library::savedPosition(96999, 100000), qint64(96999));
        QCOMPARE(Library::savedPosition(-5, 100000), qint64(0));
        QCOMPARE(Library::savedPosition(150000, 0), qint64(150000)); // unknown length: kept
        QCOMPARE(Library::savedPosition(42000, -1), qint64(42000));
    }

    void naturalSortOrdersNumbersByValue() {
        QCOMPARE(Links::naturalMediaKey("Ep 007"), QString("ep ") + QString(19, '0') + "7");
        QCOMPARE(Links::naturalMediaKey("a0b"), QString("a") + QString(20, '0') + "b");
        QStringList names{"Show E10.mkv", "show e2.mkv", "Show E1.mkv", "Show E02b.mkv"};
        std::sort(names.begin(), names.end(), [](const QString &a, const QString &b) {
            return Links::naturalMediaKey(a) < Links::naturalMediaKey(b);
        });
        QCOMPARE(names, (QStringList{"Show E1.mkv", "show e2.mkv", "Show E02b.mkv", "Show E10.mkv"}));
        QVERIFY(Links::isExtraVideo("Show/Sample/show.sample.mkv"));
        QVERIFY(Links::isExtraVideo("Behind the Scenes.mp4"));
        QVERIFY(!Links::isExtraVideo("Show S01E01.mkv"));
    }

    void nextEpisodeFollowsNaturalOrder() {
        Library::Entry e;
        e.id = "t";
        e.files = QJsonArray{file(0, "Show/Show S01E10.mkv", 100), file(1, "Show/Show S01E2.mkv", 100),
                             file(2, "Show/Show S01E1.mkv", 100), file(3, "Show/sample.mkv", 10),
                             file(4, "Show/Show S01E3.mkv", 100), file(5, "Show/Show S01E2.srt", 1)};
        e.selected = {0, 1, 2, 3, 5};
        QCOMPARE(Library::nextVideoAfter(e, 2), 1);  // E1 -> E2
        QCOMPARE(Library::nextVideoAfter(e, 1), 0);  // E3 is not selected and not downloaded
        QCOMPARE(Library::nextVideoAfter(e, 0), -1); // last
        // A downloaded unselected file counts.
        e.liveFiles = QJsonArray{QJsonObject{{"index", 4}, {"length", 100}, {"verifiedBytes", 100}}};
        QCOMPARE(Library::nextVideoAfter(e, 1), 4);
        // Warming skips a finished file: nothing left to fetch.
        QCOMPARE(Library::nextEpisodeToWarm(e, 1, true), -1);
        QCOMPARE(Library::nextEpisodeToWarm(e, 2, true), 1);
        QCOMPARE(Library::nextEpisodeToWarm(e, 2, false), -1);
        QCOMPARE(Library::nextEpisodeToWarm(e, 5, true), -1); // a subtitle is not playing
        // Watched to the end already: no next.
        e.durations.insert("1", 60000.0);
        e.positions.insert("1", 58000.0);
        QCOMPARE(Library::nextVideoAfter(e, 2), -1);
    }

    void playBodyCarriesAndroidFlags() {
        Library::Entry e;
        e.id = "t";
        e.files = QJsonArray{file(0, "S01E01.mkv", 1000), file(1, "S01E02.mkv", 1000)};
        e.selected = {0, 1};
        e.positions.insert("0", 30000.0);
        e.durations.insert("0", 60000.0);
        const QJsonObject policy{{"boost", true}, {"fileOnly", true}, {"adaptiveBuffer", true},
                                 {"burstAwareBuffer", false}, {"playNext", true}};
        QJsonObject body = Library::playBody(e, 0, policy);
        QCOMPARE(body.value("id").toString(), QString("t"));
        QCOMPARE(body.value("fileIndex").toInt(), 0);
        QCOMPARE(qint64(body.value("offset").toDouble()), qint64(500));
        QVERIFY(body.value("boost").toBool());
        QVERIFY(body.value("fileOnly").toBool());
        QVERIFY(body.value("adaptiveBuffer").toBool());
        QVERIFY(!body.contains("burstAwareBuffer"));
        QCOMPARE(body.value("nextFileIndex").toInt(), 1);
        // Direct Play never asks for file-only; no duration known, no offset.
        e.directPlay = true;
        body = Library::playBody(e, 1, policy);
        QVERIFY(!body.contains("fileOnly"));
        QVERIFY(!body.contains("offset"));
        QVERIFY(!body.contains("nextFileIndex"));
        // The shared policy's play flags are what the app sends.
        const QJsonObject shared = Policy::instance().play();
        QVERIFY(shared.contains("boost") && shared.contains("preloadStart") && shared.contains("playNext"));
    }

    void idleDropAndActiveTransfers() {
        Library::Entry e;
        e.id = "t";
        e.directPlay = true;
        e.idleDropReady = true;
        QVERIFY(Library::shouldDropIdle(e, 15, false));
        QVERIFY(!Library::shouldDropIdle(e, 0, false));  // off
        QVERIFY(!Library::shouldDropIdle(e, 15, true));  // playing here
        e.playbackReaders = 1;
        QVERIFY(!Library::shouldDropIdle(e, 15, false));
        e.playbackReaders = 0;
        e.directPlay = false;
        QVERIFY(!Library::shouldDropIdle(e, 15, false)); // downloads are never dropped

        Library::Entry running, paused, done, seeding;
        paused.paused = true;
        done.progress = 1;
        seeding.progress = 1;
        seeding.seeding = true;
        QCOMPARE(Library::activeTransfers({}), 1);
        QCOMPARE(Library::activeTransfers({running, paused, done, seeding}), 2);
    }

    // --- piece heat grid (PieceHeatGridModelTest.kt) ---

    void playbackBandsPreserveMixedBucketPositionsAndHighestPriority() {
        using namespace PieceHeat;
        const Bucket bucket = normalize(0, 10, 19, 10, 10, 4, 2);
        const auto bands = playbackBands(bucket, {{8, 11, "U"}, {10, 15, "R"}, {14, 17, "L"}, {30, 40, "U"}});
        auto sum = [&](const QString &p) {
            int n = 0;
            for (const Band &b : bands)
                if (p.isEmpty() || b.priority == p)
                    n += b.pieces;
            return n;
        };
        QCOMPARE(sum({}), 8);
        QCOMPARE(sum("U"), 2);
        QCOMPARE(sum("R"), 4);
        QCOMPARE(sum("L"), 2);
        QVERIFY(qAbs(bands.first().startFraction) < 0.001);
        double fractions = 0;
        for (const Band &b : bands)
            fractions += b.fraction;
        QVERIFY(qAbs(fractions - 0.8) < 0.001);
        QCOMPARE(bucket.selectedVerified, 4); // priorities do not change completion
    }

    void playbackBandsDoNotFillGapsOrExcludedCells() {
        using namespace PieceHeat;
        const Bucket bucket = normalize(0, 0, 9, 10, 10, 0, 0);
        const QList<Range> ranges{{1, 2, "U"}, {7, 8, "L"}};
        const auto bands = playbackBands(bucket, ranges);
        QCOMPARE(bands.size(), 2);
        QCOMPARE(bands[0].pieces + bands[1].pieces, 4);
        QVERIFY(qAbs(bands[0].startFraction - 0.1) < 0.001);
        QVERIFY(qAbs(bands[1].startFraction - 0.7) < 0.001);
        QVERIFY(playbackBands(normalize(0, 0, 9, 10, 0, 0, 0), ranges).isEmpty());
        QVERIFY(playbackBands(bucket, {}).isEmpty());
    }

    void normalizeClampsSelectedAndDisjointCounts() {
        const auto b = PieceHeat::normalize(0, 0, 7, 8, 20, 9, 5);
        QCOMPARE(b.total, 8);
        QCOMPARE(b.selected, 8);
        QCOMPARE(b.selectedVerified, 8);
        QCOMPARE(b.selectedReceiving, 0);
        QCOMPARE(b.missing, 0);
        QCOMPARE(b.excluded, 0);
        QVERIFY(!b.mixedFill());
        QVERIFY(!b.excludedOnly());
    }

    void normalizeMixedFractionsDivideByTotalNotSelected() {
        const auto b = PieceHeat::normalize(2, 16, 23, 8, 6, 3, 2);
        QCOMPARE(b.missing, 1);
        QCOMPARE(b.excluded, 2);
        QCOMPARE(b.verifiedFraction(), 0.375);
        QCOMPARE(b.receivingFraction(), 0.25);
        QCOMPARE(b.missingFraction(), 0.125);
        QCOMPARE(b.excludedFraction(), 0.25);
        QVERIFY(b.mixedFill());
        QCOMPARE(b.verifiedFraction() + b.receivingFraction() + b.missingFraction() + b.excludedFraction(), 1.0);
    }

    void excludedOnlyStaysHollowAndReceivingClampsToRemaining() {
        const auto excluded = PieceHeat::normalize(0, 0, 3, 4, 0, 2, 1);
        QVERIFY(excluded.excludedOnly());
        QCOMPARE(excluded.selectedVerified, 0);
        QCOMPARE(excluded.selectedReceiving, 0);
        QCOMPARE(excluded.excluded, 4);
        QCOMPARE(excluded.excludedFraction(), 1.0);
        const auto partial = PieceHeat::normalize(1, 4, 7, 4, 3, 2, 9);
        QCOMPARE(partial.selectedVerified, 2);
        QCOMPARE(partial.selectedReceiving, 1);
        QCOMPARE(partial.missing, 0);
        QCOMPARE(partial.excluded, 1);
        QVERIFY(partial.mixedFill());
    }

    void summaryTitleUsesSelectedPercentAndZeroSelectedMessage() {
        QLocale::setDefault(QLocale::c());
        const PieceHeat::Summary with{100, 262144, 50, 33, 4, 13, 50};
        QCOMPARE(with.verifiedPercent(), 66);
        QCOMPARE(with.titlePrimary(), QString("66% verified · 4 receiving"));
        QVERIFY(with.titleMeta().contains("100"));
        QCOMPARE(with.titleMeta(), "100 · " + Format::bytes(262144));
        const PieceHeat::Summary none{12, 4096, 0, 0, 0, 0, 12};
        QCOMPARE(none.verifiedPercent(), -1);
        QCOMPARE(none.titlePrimary(), QString("No selected pieces"));
    }

    void layoutBandsForSmallAndLargeBucketCounts() {
        using namespace PieceHeat;
        const Layout tiny = layoutFor(12);
        QCOMPARE(tiny.columns, 16);
        QCOMPARE(tiny.rows, 1);
        QVERIFY(tiny.cellDp <= 8);
        QVERIFY(tiny.heightDp() > 0);
        const Layout mid = layoutFor(100);
        QCOMPARE(mid.columns, 24);
        QCOMPARE(mid.rows, 5);
        const Layout large = layoutFor(256);
        QCOMPARE(large.columns, 32);
        QCOMPARE(large.rows, 8);
        QCOMPARE(large.cellCount, 256);
        QCOMPARE(large.cellDp, 5.0);
        const Layout capped = layoutFor(999);
        QCOMPARE(capped.columns, 32);
        QCOMPARE(capped.cellCount, MaxCells);
        QCOMPARE(layoutFor(0).cellCount, 0);
    }

    void heightFillsWidthWithSquares() {
        using namespace PieceHeat;
        const Layout layout = layoutFor(64);
        const double narrow = heightFor(layout, 200, 2);
        const double wide = heightFor(layout, 1200, 2);
        QVERIFY(narrow > 0);
        QVERIFY(wide > narrow);
        const auto [cell, gap] = squareCell(400, narrow, layout);
        QVERIFY(qAbs(layout.columns * cell + (layout.columns - 1) * gap - 400) < 0.5);
        QVERIFY(wide >= layout.cellDp);
    }

    void hitTestMapsInclusiveCellsAndMissesGaps() {
        using namespace PieceHeat;
        QCOMPARE(layoutFor(8).columns, 16);
        QCOMPARE(layoutFor(8).rows, 1);
        const Layout custom{4, 2, 8, 10, 2};
        const double w = 46, h = 22;
        QCOMPARE(hitTest(1, 1, w, h, custom), 0);
        QCOMPARE(hitTest(40, 1, w, h, custom), 3);
        QCOMPARE(hitTest(1, 13, w, h, custom), 4);
        QCOMPARE(hitTest(40, 13, w, h, custom), 7);
        QCOMPARE(hitTest(11, 1, w, h, custom), -1);  // gap
        QCOMPARE(hitTest(-1, 1, w, h, custom), -1);
        QCOMPARE(hitTest(1, 100, w, h, custom), -1);
        const Layout sparse{4, 2, 5, 10, 2};
        QCOMPARE(hitTest(1, 13, w, h, sparse), 4);
        QCOMPARE(hitTest(40, 13, w, h, sparse), -1);
    }

    void fileBoundariesMarkStartsSharedPieceAndLimitLabels() {
        using namespace PieceHeat;
        QList<Bucket> buckets;
        for (int i = 0; i < 8; ++i)
            buckets.append(normalize(i, i, i, 1, 1, 0, 0));
        const QList<FileSpan> files{{0, "alpha.mkv", 150}, {1, "beta.mkv", 50}, {2, "gamma.mkv", 50},
                                    {3, "delta.mkv", 1},   {4, "epsilon.mkv", 400}};
        const auto limited = fileBoundaryMarkers(files, 100, buckets, 2);
        QCOMPARE(limited.size(), 3);
        QCOMPARE(limited[0].bucketIndex, 0);
        QCOMPARE(limited[1].bucketIndex, 1);
        QCOMPARE(limited[2].bucketIndex, 2);
        QCOMPARE(limited[0].label, QString("alpha.mkv"));
        QCOMPARE(limited[1].label, QString("beta.mkv"));
        QVERIFY(limited[2].shared);
        QVERIFY(limited[2].label.isNull());
        const auto labeled = fileBoundaryMarkers(files, 100, buckets, 3);
        QVERIFY(labeled[2].shared);
        QVERIFY(labeled[2].label.contains("shared"));
        QVERIFY(labeled[2].label.contains("gamma"));
    }

    void peekLineUsesInclusiveRange() {
        QCOMPARE(PieceHeat::peekLine(PieceHeat::normalize(7, 56, 63, 8, 8, 3, 2)),
                 QString("Bucket 7 · pieces 56–63 · 3 verified · 2 receiving · 3 missing"));
    }

    void accessibilityMentionsEmptySelection() {
        const QString text = PieceHeat::accessibleSummary({10, 1024, 0, 0, 0, 0, 10});
        QVERIFY(text.contains("No selected pieces"));
        QVERIFY(text.contains("excluded"));
    }

    void playbackBufferCountsReadyAroundThePlayhead() {
        using namespace PieceHeat;
        // Pieces 100..107: v v s [v] v r m v, playhead at 103.
        const BufferModel m = bufferModel(100, "vvsvvrmv", 103, {{100, 102, "B"}, {103, 104, "U"}, {105, 107, "R"}});
        QCOMPARE(m.readyAhead, 2);
        QCOMPARE(m.readyBehind, 3);
        QCOMPARE(m.tiles.size(), 8);
        QVERIFY(m.tiles[3].playhead);
        QCOMPARE(m.tiles[3].tier, QString("U"));
        QVERIFY(m.tiles[2].savedOnly());
        QCOMPARE(m.tiers.size(), 3);
        QCOMPARE(m.tiers[0].tier, QString("U"));
        QCOMPARE(m.tiers[1].tier, QString("R"));
        QCOMPARE(m.tiers[1].ready, 1);
        QCOMPARE(m.tiers[1].total, 3);
        // Long windows group pieces evenly.
        QCOMPARE(bufferModel(0, QString(1000, 'v'), 0, {}).piecesPerTile, 2);
    }

    void parsesPiecesReply() {
        const QJsonObject json = QJsonDocument::fromJson(R"({"id":"e","totalPieces":20,"pieceLength":1024,
            "buckets":[{"start":0,"end":9,"total":10,"selected":10,"selectedVerified":4,"selectedReceiving":2},
                       {"start":10,"end":19,"total":10,"selected":0}],
            "playback":[{"start":2,"end":5,"priority":"U"},{"start":1,"end":1,"priority":"Z"}],
            "playheads":[3],"playbackDetail":{"start":2,"states":"vvrm","playhead":3}})").object();
        const PieceHeat::Map map = PieceHeat::parse(json);
        QCOMPARE(map.buckets.size(), 2);
        QCOMPARE(map.buckets[0].missing, 4);
        QVERIFY(map.buckets[1].excludedOnly());
        QCOMPARE(map.playback.size(), 1); // unknown tiers dropped
        QCOMPARE(map.playheads, QList<int>{3});
        QVERIFY(map.hasDetail);
        const PieceHeat::Summary s = PieceHeat::summarize(map);
        QCOMPARE(s.selectedVerified, 4);
        QCOMPARE(s.excluded, 10);
        PieceGrid grid;
        grid.setPieces(json.toVariantMap());
        QVERIFY(grid.hasMap());
        QCOMPARE(grid.summary().value("verifiedPercent").toInt(), 40);
        QVERIFY(grid.summary().value("bufferLine").toString().startsWith("Ready ahead"));
    }

    // --- player stats line (PlaybackStatsLineTest.kt, PlayerActivity info line) ---

    void statsLineMatchesAndroid() {
        QLocale::setDefault(QLocale::c());
        QCOMPARE(PlayerStats::fileLine(0, 0, 0), QString()); // empty until there is something to show
        QCOMPARE(PlayerStats::line(false, -1, -1, 0), QString());
        QCOMPARE(PlayerStats::line(true, 0.5, 3, 0), QString("Direct Play · streaming from RAM  ·  3 peers · 0 B/s"));
        QCOMPARE(PlayerStats::line(false, 0.426, -1, 0), QString("42% downloaded"));
        QCOMPARE(PlayerStats::buffered(1000, 1500), QString());
        QCOMPARE(PlayerStats::buffered(1000, 84000), QString("buffered 1:23"));
        QVERIFY(PlayerStats::fileLine(0.5, 2048, 0).startsWith("File 50% · "));
        QVERIFY(PlayerStats::fileLine(0.5, 2048, 100).contains("↑"));
    }

    // --- series (Series.kt, PlaybackLaunch.kt, NextEpisodeWarmTest.kt) ---

    void episodeTitlesReadLikeAndroid() {
        QCOMPARE(Series::episodeTitle("Show.Name.S01E03.1080p.WEB-DL.mkv"), QString("S01E03 · Show Name"));
        QCOMPARE(Series::episodeTitle("Pack/Show.Name.S1E3E4.mkv"), QString("S01E03-E04 · Show Name"));
        QCOMPARE(Series::episodeTitle("[Group] Show 2x05 720p.mp4"), QString("S02E05 · Show"));
        QCOMPARE(Series::episodeTitle("Sintel.2010.1080p.mkv"), QString("Sintel 2010"));
        QCOMPARE(Series::episodeTitle("S03E10.mkv"), QString("S03E10"));
        QCOMPARE(Series::seasonOf("Show/Show.S02E01.mkv"), 2);
        QCOMPARE(Series::seasonOf("Show/Season 3/Episode 1.mkv"), 3);
        QCOMPARE(Series::seasonOf("Sintel.mkv"), -1);
    }

    void seriesResumePicksLikeAndroid() {
        auto video = [](int index, const QString &path, qint64 pos = 0, qint64 dur = 0, qint64 at = 0) {
            Series::File f;
            f.index = index;
            f.name = path.section('/', -1);
            f.path = path;
            f.length = 100;
            f.video = true;
            f.selected = true;
            f.positionMs = pos;
            f.durationMs = dur;
            f.watchedAt = at;
            return f;
        };
        // Nothing watched: the first episode in natural order, not the largest.
        QList<Series::File> show{video(0, "S/Ep 10.mkv"), video(1, "S/Ep 2.mkv"), video(2, "S/Ep 1.mkv"),
                                 video(3, "S/Sample/Ep 0 sample.mkv")};
        QCOMPARE(Series::resumeIndex(show), 2);
        // Last watched part way: that one.
        show[1] = video(1, "S/Ep 2.mkv", 5000, 60000, 200);
        QCOMPARE(Series::continueIndex(show), 1);
        // Watched to the end: the next one.
        show[1] = video(1, "S/Ep 2.mkv", 0, 60000, 200);
        QCOMPARE(Series::continueIndex(show), 0);
        QCOMPARE(Series::resumeIndex(show), 0);
        // The last one finished: nothing to continue; an older bookmark is resumed.
        show[0] = video(0, "S/Ep 10.mkv", 0, 60000, 300);
        show[2] = video(2, "S/Ep 1.mkv", 7000, 60000, 100);
        QCOMPARE(Series::continueIndex(show), -1);
        QCOMPARE(Series::resumeFile(show), 2);
        QCOMPARE(Series::resumeIndex(show), 2);
        // A single movie: the largest file.
        Series::File movie = video(0, "M/movie.mkv");
        movie.length = 1000;
        QCOMPARE(Series::pickPlayIndex({video(1, "M/Sample/sample.mkv"), movie}), 0);
    }

    void upNextLabelsOnlyTheUpcomingRow() {
        const QJsonObject planned{{"fileIndex", 1}, {"active", false}, {"ready", false}};
        QCOMPARE(Series::upNextLabel({}, 1), QString());
        QCOMPARE(Series::upNextLabel(planned, 0), QString());
        QCOMPARE(Series::upNextLabel(planned, 1), QString("Up next · loads near the end"));
        QJsonObject active = planned;
        active["active"] = true;
        QCOMPARE(Series::upNextLabel(active, 1), QString("Up next · loading start"));
        active["ready"] = true;
        QCOMPARE(Series::upNextLabel(active, 1), QString("Up next · start loaded"));
    }

    void torrentPageGroupsBySeasonWithExtrasLast() {
        auto f = [](int index, const QString &path, bool video = true) {
            Series::File file;
            file.index = index;
            file.name = path.section('/', -1);
            file.path = path;
            file.video = video;
            return file;
        };
        const QList<Series::File> files{f(0, "Show/Featurettes/Making Of.mkv"), f(1, "Show/Show.S02E01.mkv"),
                                        f(2, "Show/Show.S01E10.mkv"), f(3, "Show/Show.S01E02.mkv"),
                                        f(4, "Show/readme.txt", false), f(5, "Show/Show.S01E02.en.srt", false)};
        QCOMPARE(Series::displayOrder(files), (QList<int>{5, 3, 2, 1, 4, 0}));
        QCOMPARE(Series::groupOf(files[0]), QString("x"));
        QCOMPARE(Series::groupOf(files[1]), QString("s2"));
        QCOMPARE(Series::groupOf(files[4]), QString());
    }

    void playlistIsTheNaturalEpisodeRun() {
        Library::Entry e;
        e.files = QJsonArray{file(0, "S/Episode 2.mkv", 100), file(1, "S/Episode 10.mkv", 100),
                             file(2, "S/Episode 1.mkv", 100), file(3, "S/notes.txt", 1),
                             file(4, "S/Sample.mkv", 10), file(5, "S/Episode 3.mkv", 100)};
        e.selected = {0, 1, 2, 3, 4};
        QCOMPARE(Library::playlist(e, 0), (QList<int>{2, 0, 1}));
        QCOMPARE(Library::playlist(e, 4), QList<int>{4}); // a sample plays alone
        // NextEpisodeWarmTest: the natural next episode, only with Play next.
        QCOMPARE(Library::nextEpisodeToWarm(e, 2, true), 0);
        QCOMPARE(Library::nextEpisodeToWarm(e, 0, true), 1);
        QCOMPARE(Library::nextEpisodeToWarm(e, 1, true), -1);
        QCOMPARE(Library::nextEpisodeToWarm(e, 2, false), -1);
        QCOMPARE(Library::nextEpisodeToWarm(e, 3, true), -1);
        // Continue watching follows the saved watch times.
        e.positions = {{"0", 0.0}};
        e.durations = {{"0", 60000.0}};
        e.watchedAt = {{"0", 5.0}};
        QCOMPARE(Series::resumeIndex(Library::seriesFiles(e)), 1);
        const Library::Entry back = Library::fromJson(Library::toJson(e));
        QCOMPARE(back.watchedAt.value("0").toDouble(), 5.0);
    }

    void directPlayIsSavedAndStartsDropped() {
        Library::Entry e;
        e.key = "dp";
        e.directPlay = true;
        e.durations = {{"0", 60000.0}};
        const Library::Entry back = Library::fromJson(Library::toJson(e));
        QVERIFY(back.directPlay);
        QCOMPARE(back.durations.value("0").toDouble(), 60000.0);
        QVERIFY(!Library::fromJson(QJsonObject{{"key", "old"}}).directPlay);

        QTemporaryDir dir;
        Engine engine(dir.filePath("engine"));
        {
            Library library(&engine, dir.filePath("data"), dir.filePath("save"));
            library.adopt("id", "hash2", "magnet:?xt=urn:btih:hash2", "Movie", QJsonArray{file(0, "Movie.mkv", 10)},
                          {0}, {}, true);
            QVERIFY(library.entry("hash2")->directPlay);
            QVERIFY(library.details("hash2").value("directPlay").toBool());
            library.savePosition("hash2", 0, 59000, 60000); // within 3 s of the end
            QCOMPARE(library.entry("hash2")->positions.value("0").toDouble(), 0.0);
            library.savePosition("hash2", 0, 20000);       // keeps the known length
            QCOMPARE(library.entry("hash2")->durations.value("0").toDouble(), 60000.0);
            QCOMPARE(library.entry("hash2")->positions.value("0").toDouble(), 20000.0);
        }
        Library reloaded(&engine, dir.filePath("data"), dir.filePath("save"));
        QVERIFY(reloaded.entry("hash2")->directPlay);
        QVERIFY(reloaded.entry("hash2")->paused); // dropped until played, like Android
    }
};

QTEST_MAIN(TorrentPlayerTests)
#include "torrentplayer_tests.moc"
