QT += core gui quick quickcontrols2 testlib dbus network multimedia concurrent
CONFIG += c++17 testcase
TARGET = torrentplayer_tests
TEMPLATE = app

INCLUDEPATH += ../src

HEADERS += ../src/engine.h ../src/library.h ../src/addsession.h ../src/subtitles.h ../src/format.h \
           ../src/links.h ../src/theme.h ../src/translator.h ../src/playback.h
SOURCES += torrentplayer_tests.cpp ../src/engine.cpp ../src/library.cpp ../src/addsession.cpp \
           ../src/subtitles.cpp ../src/format.cpp ../src/links.cpp ../src/theme.cpp ../src/translator.cpp ../src/playback.cpp
DEFINES += SRCDIR=\\\"$$PWD\\\"
