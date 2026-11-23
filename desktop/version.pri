# The app version, from Android's versionName (as desktop/bin/build does on Linux).
# Sets TP_VERSION and writes app_version.h into the build folder when it is missing.
TP_GRADLE_LINES = $$cat($$PWD/../android/app/build.gradle.kts, lines)
for (tp_line, TP_GRADLE_LINES) {
    contains(tp_line, "^ *versionName *=.*") {
        TP_VERSION = $$replace(tp_line, "[^0-9.]", "")
    }
}
isEmpty(TP_VERSION): TP_VERSION = 0.0.0
!exists($$OUT_PWD/app_version.h) {
    TP_VERSION_HEADER = "$${LITERAL_HASH}define APP_VERSION \"$$TP_VERSION\""
    write_file($$OUT_PWD/app_version.h, TP_VERSION_HEADER)
}
