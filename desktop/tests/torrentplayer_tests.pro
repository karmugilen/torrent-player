QT += core gui quick quickcontrols2 testlib dbus network multimedia concurrent
CONFIG += c++17 testcase
TARGET = torrentplayer_tests
TEMPLATE = app

INCLUDEPATH += ../src

HEADERS += ../src/engine.h ../src/library.h ../src/addsession.h ../src/subtitles.h ../src/format.h \
           ../src/links.h ../src/theme.h ../src/translator.h \
           ../src/policy.h ../src/settings.h ../src/connections.h ../src/worldmap.h ../src/playback.h \
           ../src/series.h ../src/piecegrid.h ../src/playerstats.h
SOURCES += torrentplayer_tests.cpp ../src/engine.cpp ../src/library.cpp ../src/addsession.cpp \
           ../src/subtitles.cpp ../src/format.cpp ../src/links.cpp ../src/theme.cpp ../src/translator.cpp \
           ../src/policy.cpp ../src/settings.cpp ../src/connections.cpp ../src/worldmap.cpp ../src/playback.cpp \
           ../src/series.cpp ../src/piecegrid.cpp ../src/playerstats.cpp
RESOURCES += ../src/resources.qrc
DEFINES += SRCDIR=\\\"$$PWD\\\"
QT += quick
