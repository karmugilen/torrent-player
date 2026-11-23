QT += core gui widgets qml quick quickcontrols2 network multimedia concurrent opengl

CONFIG += c++17 release
TARGET = torrentplayer
TEMPLATE = app

# Linux (and other freedesktop systems): D-Bus for the portal, UPower and NetworkManager;
# libmpv from pkg-config.
unix:!macx {
    QT += dbus
    CONFIG += link_pkgconfig
    PKGCONFIG += mpv
    HEADERS += src/portalfilepicker.h
    SOURCES += src/portalfilepicker.cpp
}

# Windows and macOS: libmpv from MPV_PREFIX (qmake6 MPV_PREFIX=<dir>): headers in
# <dir>/include/mpv, the library in <dir>/lib.
win32|macx {
    isEmpty(MPV_PREFIX): error("Set MPV_PREFIX: qmake6 MPV_PREFIX=<dir with include/mpv and lib>")
    INCLUDEPATH += $$MPV_PREFIX/include
    include(version.pri)
    VERSION = $$TP_VERSION
}
win32 {
    exists($$MPV_PREFIX/lib/mpv.lib): LIBS += $$MPV_PREFIX/lib/mpv.lib
    else: LIBS += -L$$MPV_PREFIX/lib -lmpv
    RC_ICONS = src/icons/torrentplayer.ico
    QMAKE_TARGET_PRODUCT = "Torrent Player"
    QMAKE_TARGET_DESCRIPTION = "Torrent Player"
    # A GUI app, no console window.
    CONFIG += windows
}
macx {
    LIBS += -L$$MPV_PREFIX/lib -lmpv -framework IOKit -framework CoreFoundation
    QMAKE_RPATHDIR += $$MPV_PREFIX/lib @executable_path/../Frameworks
    QMAKE_MACOSX_DEPLOYMENT_TARGET = 12.0
    ICON = src/icons/torrentplayer.icns
    QMAKE_INFO_PLIST = src/macos/Info.plist
}

HEADERS += \
    src/engine.h \
    src/library.h \
    src/addsession.h \
    src/subtitles.h \
    src/format.h \
    src/links.h \
    src/filepicker.h \
    src/nativefilepicker.h \
    src/platform.h \
    src/theme.h \
    src/translator.h \
    src/policy.h \
    src/settings.h \
    src/connections.h \
    src/worldmap.h \
    src/mpvitem.h \
    src/playback.h \
    src/battery.h \
    src/autostart.h \
    src/tray.h \
    src/series.h \
    src/piecegrid.h \
    src/playerstats.h \
    src/pipwindow.h

SOURCES += \
    src/main.cpp \
    src/engine.cpp \
    src/library.cpp \
    src/librarysettings.cpp \
    src/addsession.cpp \
    src/subtitles.cpp \
    src/format.cpp \
    src/links.cpp \
    src/nativefilepicker.cpp \
    src/platform.cpp \
    src/theme.cpp \
    src/translator.cpp \
    src/policy.cpp \
    src/settings.cpp \
    src/connections.cpp \
    src/worldmap.cpp \
    src/mpvitem.cpp \
    src/playback.cpp \
    src/battery.cpp \
    src/autostart.cpp \
    src/tray.cpp \
    src/series.cpp \
    src/piecegrid.cpp \
    src/playerstats.cpp \
    src/pipwindow.cpp

RESOURCES += src/resources.qrc
