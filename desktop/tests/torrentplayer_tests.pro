QT += core gui widgets quick quickcontrols2 testlib network multimedia concurrent
unix:!macx: QT += dbus
macx: LIBS += -framework IOKit -framework CoreFoundation
CONFIG += c++17 testcase
TARGET = torrentplayer_tests
TEMPLATE = app

INCLUDEPATH += ../src

HEADERS += ../src/engine.h ../src/library.h ../src/addsession.h ../src/subtitles.h ../src/format.h \
           ../src/links.h ../src/theme.h ../src/translator.h \
           ../src/policy.h ../src/settings.h ../src/connections.h ../src/worldmap.h ../src/playback.h \
           ../src/battery.h ../src/autostart.h ../src/series.h ../src/piecegrid.h ../src/playerstats.h \
           ../src/platform.h ../src/nativefilepicker.h ../src/filepicker.h ../src/pipwindow.h
SOURCES += torrentplayer_tests.cpp ../src/engine.cpp ../src/library.cpp ../src/librarysettings.cpp \
           ../src/addsession.cpp ../src/subtitles.cpp ../src/format.cpp ../src/links.cpp ../src/theme.cpp \
           ../src/translator.cpp ../src/policy.cpp ../src/settings.cpp ../src/connections.cpp \
           ../src/worldmap.cpp ../src/playback.cpp ../src/battery.cpp ../src/autostart.cpp \
           ../src/series.cpp ../src/piecegrid.cpp ../src/playerstats.cpp \
           ../src/platform.cpp ../src/nativefilepicker.cpp ../src/pipwindow.cpp
RESOURCES += ../src/resources.qrc
DEFINES += SRCDIR=\\\"$$PWD\\\"
QT += quick
