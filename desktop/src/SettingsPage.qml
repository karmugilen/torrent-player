import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Page {
    id: page
    readonly property string pageTitle: qsTr("Settings")
    background: Rectangle { color: "#0e0e10" }

    function optionLabel(key, value) {
        const opts = settings.options(key)
        for (let i = 0; i < opts.length; ++i) {
            if (opts[i] === value) {
                // Desktop wording for the built-in / mpv choice.
                if (key === "builtInPlayer")
                    return value ? qsTr("Built-in player") : qsTr("mpv window")
                const labels = {
                    "directPlayDefault": value ? qsTr("Direct Play") : qsTr("Download"),
                    "idleDropMinutes": value === 0 ? qsTr("Off") : value === 5 ? qsTr("5 min") : value === 15 ? qsTr("15 min") : value === 30 ? qsTr("30 min") : String(value),
                    "networkPolicy": value === "ANY" ? qsTr("Any network") : value === "WIFI_ONLY" ? qsTr("Wi-Fi") : qsTr("Unmetered"),
                    "downloadRateKiB": value === 0 ? qsTr("None") : value === 1024 ? qsTr("1 MB/s") : value === 4096 ? qsTr("4 MB/s") : value === 8192 ? qsTr("8 MB/s") : String(value),
                    "seedRatioMilli": value === 500 ? qsTr("0.5×") : value === 1000 ? qsTr("1×") : value === 2000 ? qsTr("2×") : String(value),
                    "seedTimeMinutes": value === 15 ? qsTr("15 min") : value === 60 ? qsTr("1 hour") : value === 180 ? qsTr("3 hours") : String(value),
                    "seedUploadKiB": value + " KiB/s"
                }
                if (labels[key])
                    return labels[key]
            }
        }
        if (key === "builtInPlayer")
            return value ? qsTr("Built-in player") : qsTr("mpv window")
        return String(value)
    }

    header: RowLayout {
        width: parent.width
        Layout.margins: 0
        spacing: 8
        Button {
            flat: true
            text: qsTr("Back")
            onClicked: stack.pop()
        }
        Label {
            text: qsTr("Settings")
            font.pixelSize: 22
            font.weight: Font.DemiBold
            color: win.textColor
            Layout.fillWidth: true
        }
    }

    Flickable {
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.height + 48
        clip: true
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: column
            width: Math.min(720, parent.width - 48)
            x: (parent.width - width) / 2
            y: 8
            spacing: 20

            // ---- Downloads ----
            SettingsGroup {
                title: qsTr("Downloads")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 10
                    RowLayout {
                        Layout.fillWidth: true
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Label { text: qsTr("Save folder"); color: win.textColor; font.pixelSize: 15 }
                            Label {
                                text: library.saveDirectory
                                color: win.muted
                                font.pixelSize: 13
                                wrapMode: Text.Wrap
                                Layout.fillWidth: true
                            }
                        }
                        Button {
                            text: qsTr("Change")
                            flat: true
                            onClicked: picker.open("directory")
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        color: win.muted
                        font.pixelSize: 12
                        text: library.freeBytes() >= 0
                              ? qsTr("%1 used by downloads · %2 free").arg(fmt.bytes(library.managedBytes())).arg(fmt.bytes(library.freeBytes()))
                              : qsTr("Free space unavailable")
                    }
                    ChoiceSetting {
                        title: qsTr("When adding a torrent")
                        supporting: settings.directPlayDefault
                                    ? qsTr("Direct Play streams the video from RAM and saves only what makes it open fast. Both stay available each time.")
                                    : qsTr("Download saves the selected files for offline play. Both stay available each time.")
                        options: settings.options("directPlayDefault")
                        selected: settings.directPlayDefault
                        labelFor: (v) => page.optionLabel("directPlayDefault", v)
                        onSelect: (v) => { settings.directPlayDefault = v }
                    }
                }
            }

            // ---- Playback ----
            SettingsGroup {
                title: qsTr("Playback")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    ChoiceSetting {
                        title: qsTr("Stop idle Direct Play")
                        supporting: qsTr("Frees memory when the player is closed and nothing has played for this long. Play starts it again.")
                        options: settings.options("idleDropMinutes")
                        selected: settings.idleDropMinutes
                        labelFor: (v) => page.optionLabel("idleDropMinutes", v)
                        onSelect: (v) => { settings.idleDropMinutes = v }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    ChoiceSetting {
                        title: qsTr("How players get the video")
                        supporting: settings.builtInPlayer
                                    ? qsTr("Built-in player: plays inside this app and fetches the spot you seek to first.")
                                    : qsTr("mpv window: opens the stream in mpv in its own window.")
                        options: settings.options("builtInPlayer")
                        selected: settings.builtInPlayer
                        labelFor: (v) => page.optionLabel("builtInPlayer", v)
                        onSelect: (v) => { settings.builtInPlayer = v }
                    }
                }
            }

            // ---- Network ----
            SettingsGroup {
                title: qsTr("Network")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    ChoiceSetting {
                        title: qsTr("Transfer on")
                        supporting: qsTr("Torrent data waits until the network matches.")
                        options: settings.options("networkPolicy")
                        selected: settings.networkPolicy
                        labelFor: (v) => page.optionLabel("networkPolicy", v)
                        onSelect: (v) => { settings.networkPolicy = v }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    ChoiceSetting {
                        title: qsTr("Download speed limit")
                        options: settings.options("downloadRateKiB")
                        selected: settings.downloadRateKiB
                        labelFor: (v) => page.optionLabel("downloadRateKiB", v)
                        onSelect: (v) => { settings.downloadRateKiB = v }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        RowLayout {
                            Label { text: qsTr("Peers per torrent"); color: win.textColor; font.pixelSize: 15; Layout.fillWidth: true }
                            Label {
                                text: String(settings.maxPeers)
                                color: win.textColor
                                font.weight: Font.DemiBold
                                padding: 6
                                background: Rectangle { color: win.surfaceHigh; radius: win.smallRadius }
                            }
                        }
                        Label {
                            text: qsTr("More peers can be faster and use more battery.")
                            color: win.muted
                            font.pixelSize: 12
                            Layout.fillWidth: true
                        }
                        Slider {
                            Layout.fillWidth: true
                            from: 8
                            to: 500
                            stepSize: 1
                            value: settings.maxPeers
                            onMoved: settings.maxPeers = Math.round(value)
                        }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    SwitchSetting {
                        title: qsTr("Anti-DPI")
                        supporting: settings.antiDpiEnabled
                                    ? qsTr("On for new TCP peer connections. It does not hide your address.")
                                    : qsTr("Experimental, for networks that block torrents. Not working in this version yet.")
                        checked: settings.antiDpiEnabled
                        onToggled: (v) => { settings.antiDpiEnabled = v }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    SettingRow {
                        title: qsTr("Incoming port")
                        supporting: deviceStatus.listenPort > 0
                                    ? qsTr("%1, forwarded by the router when it allows it.").arg(deviceStatus.listenPort)
                                    : qsTr("Not ready yet.")
                    }
                }
            }

            // ---- Seeding ----
            SettingsGroup {
                title: qsTr("Seeding")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    SwitchSetting {
                        title: qsTr("Seed finished downloads")
                        supporting: qsTr("Shares what you downloaded until a limit below is reached. Pauses while a video plays.")
                        checked: settings.seedingEnabled
                        onToggled: (v) => { settings.seedingEnabled = v }
                    }
                    ColumnLayout {
                        visible: settings.seedingEnabled
                        Layout.fillWidth: true
                        spacing: 0
                        Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                        ChoiceSetting {
                            title: qsTr("Stop at share ratio")
                            options: settings.options("seedRatioMilli")
                            selected: settings.seedRatioMilli
                            labelFor: (v) => page.optionLabel("seedRatioMilli", v)
                            onSelect: (v) => { settings.seedRatioMilli = v }
                        }
                        Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                        ChoiceSetting {
                            title: qsTr("Or after seeding for")
                            options: settings.options("seedTimeMinutes")
                            selected: settings.seedTimeMinutes
                            labelFor: (v) => page.optionLabel("seedTimeMinutes", v)
                            onSelect: (v) => { settings.seedTimeMinutes = v }
                        }
                        Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                        ChoiceSetting {
                            title: qsTr("Upload speed limit")
                            options: settings.options("seedUploadKiB")
                            selected: settings.seedUploadKiB
                            labelFor: (v) => page.optionLabel("seedUploadKiB", v)
                            onSelect: (v) => { settings.seedUploadKiB = v }
                        }
                        Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                        SwitchSetting {
                            title: qsTr("Only on Wi-Fi")
                            checked: settings.seedWiFiOnly
                            onToggled: (v) => { settings.seedWiFiOnly = v }
                        }
                        Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                        SwitchSetting {
                            title: qsTr("Only while charging")
                            checked: settings.seedChargingOnly
                            onToggled: (v) => { settings.seedChargingOnly = v }
                        }
                    }
                }
            }

            // ---- Battery and memory ----
            SettingsGroup {
                title: qsTr("Battery and memory")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    SwitchSetting {
                        title: qsTr("Pause on low battery")
                        supporting: deviceStatus.batteryPaused
                                    ? qsTr("Paused now: battery is under 15% and the computer is not charging.")
                                    : qsTr("Stops torrent data and playback under 15% battery unless the computer is charging.")
                        checked: settings.pauseOnLowBattery
                        onToggled: (v) => { settings.pauseOnLowBattery = v }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 6
                        RowLayout {
                            Label { text: qsTr("Memory per torrent"); color: win.textColor; font.pixelSize: 15; Layout.fillWidth: true }
                            Label {
                                text: settings.ramCacheMiB + " MiB"
                                color: win.textColor
                                font.weight: Font.DemiBold
                                padding: 6
                                background: Rectangle { color: win.surfaceHigh; radius: win.smallRadius }
                            }
                        }
                        Label {
                            Layout.fillWidth: true
                            color: win.muted
                            font.pixelSize: 12
                            text: settings.ramCacheAuto
                                  ? qsTr("Auto picks a size from free memory. Move the slider to set your own.")
                                  : qsTr("Used for downloads and Direct Play. Smaller keeps seeking quick.")
                        }
                        Slider {
                            id: ramSlider
                            Layout.fillWidth: true
                            from: 0
                            to: Math.max(0, settings.ramCacheSteps.length - 1)
                            stepSize: 1
                            snapMode: Slider.SnapAlways
                            value: {
                                const steps = settings.ramCacheSteps
                                let best = 0
                                for (let i = 0; i < steps.length; ++i)
                                    if (steps[i] <= settings.ramCacheMiB)
                                        best = i
                                return best
                            }
                            onMoved: {
                                const steps = settings.ramCacheSteps
                                const i = Math.round(value)
                                if (i >= 0 && i < steps.length)
                                    settings.ramCacheMiB = steps[i]
                            }
                        }
                        Button {
                            text: qsTr("Auto")
                            flat: true
                            highlighted: settings.ramCacheAuto
                            onClicked: settings.ramCacheAuto = true
                        }
                    }
                }
            }

            // ---- Appearance (Reduce motion only on desktop) ----
            SettingsGroup {
                title: qsTr("Appearance")
                SwitchSetting {
                    title: qsTr("Reduce motion")
                    supporting: qsTr("Fewer animations.")
                    checked: settings.reducedEffects
                    onToggled: (v) => { settings.reducedEffects = v }
                }
            }

            // ---- Data ----
            SettingsGroup {
                title: qsTr("Data")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    SwitchSetting {
                        title: qsTr("Start at login")
                        supporting: qsTr("Opens in the tray with downloads running in the background.")
                        checked: autostart.enabled
                        onToggled: (v) => { autostart.enabled = v }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    SettingRow {
                        title: qsTr("Cache")
                        supporting: qsTr("Previews and engine caches for torrents no longer in the library. Saved downloads are kept.")
                        Button {
                            text: qsTr("Clear")
                            flat: true
                            onClicked: {
                                const n = library.clearOrphanBoundaryCache()
                                win.toast(n > 0
                                          ? qsTr("Cache cleared: %1 reclaimed.").arg(fmt.bytes(n))
                                          : qsTr("Temporary files were removed."))
                            }
                        }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    SettingRow {
                        title: qsTr("Stop all transfers")
                        supporting: qsTr("Disconnects peers. Saved files are kept.")
                        Button {
                            text: qsTr("Stop")
                            flat: true
                            enabled: library.activeCount > 0 || library.count > 0
                            onClicked: {
                                library.pauseAll()
                                win.toast(qsTr("All transfers paused"))
                            }
                        }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    SettingRow {
                        title: qsTr("Shut down")
                        supporting: qsTr("Stops transfers and closes the app.")
                        Button {
                            text: qsTr("Exit")
                            flat: true
                            onClicked: win.requestQuit()
                        }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 8; Layout.bottomMargin: 8 }
                    SettingRow {
                        title: qsTr("Delete downloaded files")
                        titleColor: win.danger
                        supporting: qsTr("Erases every saved download. The library list stays.")
                        Button {
                            text: qsTr("Delete")
                            flat: true
                            Material.foreground: win.danger
                            onClicked: clearDialog.open()
                        }
                    }
                }
            }

            // ---- About ----
            SettingsGroup {
                title: qsTr("About")
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    SettingRow {
                        title: qsTr("Privacy policy")
                        supporting: qsTr("Nothing is collected. Lists every connection the app makes.")
                        onClicked: Qt.openUrlExternally("https://github.com/karmugilen/torrent-player/blob/v2.4.0/PRIVACY.md")
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 4; Layout.bottomMargin: 4 }
                    SettingRow {
                        title: qsTr("Source code")
                        supporting: qsTr("Free and open source.")
                        onClicked: Qt.openUrlExternally("https://github.com/karmugilen/torrent-player")
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 4; Layout.bottomMargin: 4 }
                    SettingRow {
                        title: qsTr("Open source licenses")
                        supporting: qsTr("Licenses of the libraries the app is built with.")
                        onClicked: Qt.openUrlExternally("https://github.com/karmugilen/torrent-player/blob/v2.4.4/NOTICE.md")
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: win.line; Layout.topMargin: 4; Layout.bottomMargin: 4 }
                    SettingRow {
                        title: qsTr("Version")
                        supporting: Qt.application.version
                    }
                }
            }
        }
    }

    Dialog {
        id: clearDialog
        anchors.centerIn: parent
        modal: true
        title: qsTr("Delete downloaded files?")
        width: Math.min(480, page.width - 64)
        padding: 24
        Material.roundedScale: theme.radius
        background: Rectangle { color: win.surface; radius: win.radius; border.color: win.line }
        Label {
            width: clearDialog.availableWidth
            wrapMode: Text.Wrap
            color: win.muted
            text: qsTr("Erases every saved download from disk. The library list stays.")
        }
        footer: DialogButtonBox {
            Button { text: qsTr("Cancel"); flat: true; DialogButtonBox.buttonRole: DialogButtonBox.RejectRole }
            Button {
                text: qsTr("Delete")
                flat: true
                Material.foreground: win.danger
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }
        onAccepted: library.deleteDownloadedFiles()
    }

    // --- small building blocks ---
    component SettingsGroup: ColumnLayout {
        property string title
        default property alias content: body.data
        spacing: 8
        Layout.fillWidth: true
        Label {
            text: title
            color: theme.accent
            font.pixelSize: 13
            font.weight: Font.DemiBold
            leftPadding: 4
        }
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: body.implicitHeight + 24
            color: win.surface
            radius: win.radius
            border.color: win.line
            ColumnLayout {
                id: body
                width: parent.width - 32
                x: 16
                y: 12
                spacing: 8
            }
        }
    }

    component SettingRow: RowLayout {
        property string title
        property string supporting
        property color titleColor: win.textColor
        default property alias trailing: trailingSlot.data
        signal clicked()
        Layout.fillWidth: true
        spacing: 12
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Label { text: title; color: titleColor; font.pixelSize: 15 }
            Label {
                visible: supporting.length > 0
                text: supporting
                color: win.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
        }
        Row {
            id: trailingSlot
            spacing: 4
            Layout.alignment: Qt.AlignVCenter
        }
        MouseArea {
            anchors.fill: parent
            enabled: trailingSlot.children.length === 0
            z: -1
            onClicked: parent.clicked()
        }
    }

    component SwitchSetting: RowLayout {
        property string title
        property string supporting
        property bool checked
        signal toggled(bool value)
        Layout.fillWidth: true
        spacing: 12
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2
            Label { text: title; color: win.textColor; font.pixelSize: 15 }
            Label {
                visible: supporting && supporting.length > 0
                text: supporting || ""
                color: win.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
                Layout.fillWidth: true
            }
        }
        Switch {
            checked: parent.checked
            onToggled: parent.toggled(checked)
        }
    }

    component ChoiceSetting: ColumnLayout {
        property string title
        property string supporting
        property var options: []
        property var selected
        property var labelFor: (v) => String(v)
        signal select(var value)
        Layout.fillWidth: true
        spacing: 6
        Label { text: title; color: win.textColor; font.pixelSize: 15 }
        Label {
            visible: supporting && supporting.length > 0
            text: supporting || ""
            color: win.muted
            font.pixelSize: 12
            wrapMode: Text.Wrap
            Layout.fillWidth: true
        }
        Flow {
            Layout.fillWidth: true
            spacing: 8
            Repeater {
                model: options
                delegate: Button {
                    flat: true
                    checkable: true
                    checked: modelData === selected
                    text: labelFor(modelData)
                    onClicked: select(modelData)
                    background: Rectangle {
                        radius: win.smallRadius
                        color: parent.checked ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.25) : win.surfaceHigh
                        border.color: parent.checked ? theme.accent : win.line
                    }
                }
            }
        }
    }
}
