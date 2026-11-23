#pragma once

#include <QString>

// What the user pasted, dropped, or passed on the command line.
namespace Links {

enum class Kind { Invalid, Magnet, TorrentUrl, TorrentFile };

Kind classify(const QString &input);

// The lowercase hex info hash a magnet names (v1 btih in hex or base32, or
// the v2 btmh multihash), or an empty string when it names none.
QString magnetInfoHash(const QString &magnet);

// A local path from a plain path or a file:// URL.
QString localPath(const QString &input);

// True for the video files the player can open, by extension or MIME type.
bool isVideo(const QString &name, const QString &mimeType = {});
bool isSubtitle(const QString &name);
bool isImage(const QString &name);

// A subtitle belongs to a video when its name starts with the video's base
// name ("Movie.en.srt" for "Movie.mkv"), the way the Android app pre-selects.
bool subtitleMatches(const QString &videoName, const QString &subtitleName);

// Sort key that orders numbers by value ("Ep 2" before "Ep 10"), lowercase
// (PlaybackLaunch.kt naturalMediaKey): each digit run is zero-padded to 20 places.
QString naturalMediaKey(const QString &path);

// Samples, trailers and featurettes: playable on their own, never part of an
// episode run (Series.kt isExtraVideo).
bool isExtraVideo(const QString &path);

}
