#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonObject>
#include <QTemporaryDir>
#include <QJsonDocument>
#include <QtTest>

#include "addsession.h"
#include "engine.h"
#include "format.h"
#include "library.h"
#include "links.h"
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
};

QTEST_MAIN(TorrentPlayerTests)
#include "torrentplayer_tests.moc"
