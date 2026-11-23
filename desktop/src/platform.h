#pragma once

#include <QString>
#include <QStringList>
#include <QtGlobal>

// Linux and the other freedesktop systems (D-Bus, XDG portal, UPower, NetworkManager,
// Omarchy). Windows and macOS take the native paths instead.
#if defined(Q_OS_UNIX) && !defined(Q_OS_DARWIN)
#define TORRENTPLAYER_FREEDESKTOP 1
#else
#define TORRENTPLAYER_FREEDESKTOP 0
#endif

// Where the app looks for things on each OS. Pure functions of their arguments, so
// tests on Linux can check the Windows and macOS lists too.
namespace Platform {

enum class Os { Linux, Windows, MacOS };

Os current();

// engine-host, or engine-host.exe on Windows.
QString engineBinaryName(Os os);
// TORRENTPLAYER_ENGINE, then next to the app, then ../lib/torrentplayer (Linux package
// and AppImage usr/lib/torrentplayer), then /usr/lib/torrentplayer on Linux.
QStringList engineCandidates(Os os, const QString &appDir, const QString &environmentOverride);

// Folders that may hold geoip-country*.mmdb, in search order.
QStringList geoipDirectories(Os os, const QString &appDir, const QString &environmentDir);
// Windows links need admin rights, so the data folder gets a copy there.
bool geoipCopiesFile(Os os);

// mpv for "Open in mpv". onPath is QStandardPaths::findExecutable("mpv"), maybe empty.
// Linux keeps the bare name (QProcess looks it up on PATH); macOS apps started from
// Finder don't get the shell's PATH, so the usual install places come after it.
QStringList externalMpvCandidates(Os os, const QString &onPath);

// Name of the single-instance local socket (a named pipe on Windows).
QString instanceServerName(const QString &user);

} // namespace Platform
