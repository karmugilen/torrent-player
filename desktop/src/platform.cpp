#include "platform.h"

namespace Platform {

Os current() {
#if defined(Q_OS_WIN)
    return Os::Windows;
#elif defined(Q_OS_MACOS)
    return Os::MacOS;
#else
    return Os::Linux;
#endif
}

QString engineBinaryName(Os os) {
    return os == Os::Windows ? QStringLiteral("engine-host.exe") : QStringLiteral("engine-host");
}

QStringList engineCandidates(Os os, const QString &appDir, const QString &environmentOverride) {
    const QString name = engineBinaryName(os);
    QStringList candidates;
    if (!environmentOverride.isEmpty())
        candidates << environmentOverride;
    candidates << appDir + "/" + name << appDir + "/../lib/torrentplayer/" + name;
    if (os == Os::Linux)
        candidates << "/usr/lib/torrentplayer/" + name;
    return candidates;
}

QStringList geoipDirectories(Os os, const QString &appDir, const QString &environmentDir) {
    QStringList dirs;
    if (!environmentDir.isEmpty())
        dirs << environmentDir;
    dirs << appDir << appDir + "/../share/torrentplayer";
    if (os == Os::MacOS)
        dirs << appDir + "/../Resources";
    if (os != Os::Windows)
        dirs << "/usr/share/torrentplayer";
    // A build in desktop/build reads the Android app's copy.
    dirs << appDir + "/../../android/app/src/main/assets";
    return dirs;
}

bool geoipCopiesFile(Os os) { return os == Os::Windows; }

QStringList externalMpvCandidates(Os os, const QString &onPath) {
    QStringList candidates;
    switch (os) {
    case Os::Linux:
        candidates << "mpv";
        break;
    case Os::Windows:
        if (!onPath.isEmpty())
            candidates << onPath;
        candidates << "mpv.exe";
        break;
    case Os::MacOS:
        if (!onPath.isEmpty())
            candidates << onPath;
        candidates << "/Applications/mpv.app/Contents/MacOS/mpv" << "/opt/homebrew/bin/mpv"
                   << "/usr/local/bin/mpv";
        break;
    }
    return candidates;
}

QString instanceServerName(const QString &user) {
    return QString("torrentplayer-%1").arg(user.isEmpty() ? QStringLiteral("user") : user);
}

} // namespace Platform
