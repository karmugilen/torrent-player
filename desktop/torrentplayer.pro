QT += core gui qml quick quickcontrols2 dbus network multimedia concurrent opengl

CONFIG += c++17 release link_pkgconfig
PKGCONFIG += mpv
TARGET = torrentplayer
TEMPLATE = app

HEADERS += \
    src/engine.h \
    src/library.h \
    src/addsession.h \
    src/subtitles.h \
    src/format.h \
    src/links.h \
    src/filepicker.h \
    src/portalfilepicker.h \
    src/theme.h \
    src/translator.h \
    src/policy.h \
    src/settings.h \
    src/connections.h \
    src/worldmap.h \
    src/mpvitem.h \
    src/playback.h

SOURCES += \
    src/main.cpp \
    src/engine.cpp \
    src/library.cpp \
    src/addsession.cpp \
    src/subtitles.cpp \
    src/format.cpp \
    src/links.cpp \
    src/portalfilepicker.cpp \
    src/theme.cpp \
    src/translator.cpp \
    src/policy.cpp \
    src/settings.cpp \
    src/connections.cpp \
    src/worldmap.cpp \
    src/mpvitem.cpp \
    src/playback.cpp

RESOURCES += src/resources.qrc
