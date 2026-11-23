#pragma once

#include <QJsonObject>
#include <QList>
#include <QString>

// Torrents with many videos (Android Series.kt, Formatters.kt pickPlayIndex,
// UiModels.kt resumeFile, TorrentFilesScreen.kt upNextLabel). Pure, tested.
namespace Series {

// One file as the series rules see it.
struct File {
    int index = -1;
    QString name;
    QString path;
    qint64 length = 0;
    bool video = false;
    bool selected = false;
    bool verifiedComplete = false;
    qint64 positionMs = 0;  // saved place, 0 when watched to the end or never played
    qint64 durationMs = 0;  // length of the video when the player knew it
    qint64 watchedAt = 0;   // when the place was last saved (Android bookmarkUpdatedAt), 0 never
};

// "Show.Name.S01E03.1080p.WEB-DL.mkv" -> "S01E03 · Show Name" (Series.kt episodeTitle).
QString episodeTitle(const QString &fileName);

// Season number from "S01E02", "1x02" or a "Season 1" folder, else -1.
int seasonOf(const QString &path);

// Continue for a series: the last watched episode at its place; when that one was
// watched to the end, the episode after it. -1 when nothing was watched (seriesContinueIndex).
int continueIndex(const QList<File> &files);
// Best unfinished bookmark in the torrent, most recent first (DownloadEntry.resumeFile), or -1.
int resumeFile(const QList<File> &files);
// The usual first pick: first episode in natural order, else the largest file (pickPlayIndex).
int pickPlayIndex(const QList<File> &files);
// What Play opens: continueIndex, else resumeFile, else pickPlayIndex (seriesResumeIndex).
int resumeIndex(const QList<File> &files);

// Play next warm-up state for the row of fileIndex, from the engine's nextPreload
// ({fileIndex, active, ready}); empty for other rows (upNextLabel).
QString upNextLabel(const QJsonObject &nextPreload, int fileIndex);

// The torrent page's file order: by season, then files without one, then extras;
// inside each group in natural name order. Returns file indexes.
QList<int> displayOrder(const QList<File> &files);
// Group of a file for the torrent page: "s<N>" for a season, "x" for extras, "" otherwise.
QString groupOf(const File &file);

}
