QT += core gui qml quick quickcontrols2 dbus network multimedia concurrent

CONFIG += c++17 release
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
    src/translator.h

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
    src/translator.cpp

RESOURCES += src/resources.qrc
