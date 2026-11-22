import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Page {
    id: page
    property string torrentKey
    // Re-read on every poll; the library bumps revision once a second.
    readonly property var info: library.revision >= 0 ? library.details(torrentKey) : ({})
    readonly property string pageTitle: info.name || ""
    property string filter: "all"
    property string query: ""
    readonly property var shown: visibleFiles()
    background: Rectangle { color: "#0e0e10" }

    // The library dropped it (removed from another place): go back.
    onInfoChanged: if (info.key === undefined && StackView.status === StackView.Active) StackView.view.pop()

    Shortcut { sequence: "Space"; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen && !searchField.activeFocus && info.defaultVideo >= 0; onActivated: win.play(torrentKey, info.defaultVideo) }
    Shortcut { sequence: "P"; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen && !searchField.activeFocus; onActivated: library.togglePause(torrentKey) }
    Shortcut { sequence: "Delete"; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen && !searchField.activeFocus; onActivated: win.confirmRemove(torrentKey) }
    Shortcut { sequences: ["/", "Ctrl+F"]; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen; onActivated: searchField.forceActiveFocus() }

    function visibleFiles() {
        const files = info.files || []
        const q = query.toLowerCase()
        return files.filter(f => (filter === "all" || (filter === "videos" && f.video) || (filter === "subtitles" && f.subtitle))
                                 && (q === "" || f.path.toLowerCase().indexOf(q) >= 0))
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Top bar.
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 16
            Layout.bottomMargin: 0
            spacing: 8
            ToolButton {
                text: "←"
                font.pixelSize: 20
                ToolTip.text: qsTr("Back")
                ToolTip.visible: hovered
                onClicked: page.StackView.view.pop()
            }
            Label {
                Layout.fillWidth: true
                text: info.name || ""
                elide: Text.ElideMiddle
                font.pixelSize: 20
                font.weight: Font.DemiBold
                color: win.textColor
            }
            Button {
                text: info.paused ? qsTr("Resume") : qsTr("Pause")
                flat: true
                visible: info.state !== "done" && info.state !== "error"
                onClicked: library.togglePause(torrentKey)
            }
            Button {
                text: qsTr("Retry")
                flat: true
                visible: info.state === "error"
                onClicked: library.retry(torrentKey)
            }
            Button { text: qsTr("Open folder"); flat: true; onClicked: library.openFolder(torrentKey) }
            Button {
                text: qsTr("Peers")
                flat: true
                onClicked: win.openPeers(torrentKey)
            }
            Button {
                text: qsTr("Remove…")
                flat: true
                Material.foreground: win.danger
                onClicked: win.confirmRemove(torrentKey)
            }
        }

        // Summary: picture, play, and the numbers.
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 24
            spacing: 24

            Rectangle {
                Layout.preferredWidth: 320
                Layout.preferredHeight: 180
                radius: win.radius
                color: "#111114"
                clip: true
                Image {
                    anchors.fill: parent
                    source: info.thumbnail || ""
                    fillMode: Image.PreserveAspectCrop
                    visible: status === Image.Ready
                    sourceSize.width: 640
                }
                Label {
                    anchors.centerIn: parent
                    visible: !info.thumbnail
                    text: (info.name || "?").charAt(0).toUpperCase()
                    font.pixelSize: 64
                    font.weight: Font.Bold
                    color: "#2c2c33"
                }
                RoundButton {
                    anchors.centerIn: parent
                    visible: info.defaultVideo >= 0
                    width: 72
                    height: 72
                    text: "▶"
                    font.pixelSize: 26
                    Material.background: theme.accent
                    Material.foreground: theme.accentForeground
                    onClicked: win.play(torrentKey, info.defaultVideo)
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 10
                RowLayout {
                    spacing: 12
                    Label {
                        text: win.stateLabel(info.state)
                        color: info.state === "error" ? win.danger : theme.accent
                        font.pixelSize: 15
                        font.weight: Font.Medium
                    }
                    Label {
                        text: fmt.percent(info.progress || 0)
                        color: win.textColor
                        font.pixelSize: 15
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: info.state === "error"
                    text: info.error || ""
                    color: win.danger
                    wrapMode: Text.Wrap
                }
                ProgressBar {
                    Layout.fillWidth: true
                    Layout.maximumWidth: 560
                    from: 0; to: 1
                    value: info.progress || 0
                    indeterminate: info.state === "fetching" || info.state === "starting"
                }
                GridLayout {
                    columns: 4
                    columnSpacing: 32
                    rowSpacing: 6
                    Repeater {
                        model: [
                            [qsTr("Download"), fmt.speed(info.downloadSpeed || 0)],
                            [qsTr("Upload"), fmt.speed(info.uploadSpeed || 0)],
                            [qsTr("Peers"), String(info.peers || 0)],
                            [qsTr("ETA"), info.state === "downloading" && fmt.eta(info.eta) !== "" ? fmt.eta(info.eta) : "—"],
                            [qsTr("Downloaded"), fmt.bytes(info.downloaded || 0)],
                            [qsTr("Size"), fmt.bytes(info.size || 0)],
                            [qsTr("Files"), String((info.files || []).length)],
                            [qsTr("Added"), info.addedAt ? Qt.formatDate(info.addedAt, Qt.locale(), Locale.ShortFormat) : ""]
                        ]
                        delegate: ColumnLayout {
                            spacing: 2
                            Label { text: modelData[0]; color: win.muted; font.pixelSize: 12 }
                            Label { text: modelData[1]; color: win.textColor; font.pixelSize: 14 }
                        }
                    }
                }
                RowLayout {
                    spacing: 4
                    Label {
                        text: qsTr("Info hash")
                        color: win.muted
                        font.pixelSize: 12
                    }
                    Label {
                        text: torrentKey
                        color: win.muted
                        font.family: "monospace"
                        font.pixelSize: 12
                        elide: Text.ElideMiddle
                        Layout.maximumWidth: 380
                    }
                    ToolButton {
                        text: qsTr("Copy")
                        font.pixelSize: 12
                        onClicked: { library.copyText(torrentKey); win.toast(qsTr("Info hash copied")) }
                    }
                    ToolButton {
                        text: qsTr("Copy magnet link")
                        font.pixelSize: 12
                        onClicked: { library.copyMagnet(torrentKey); win.toast(qsTr("Magnet link copied")) }
                    }
                }
            }
        }

        // Files.
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 24
            Layout.rightMargin: 24
            spacing: 8
            Repeater {
                model: [["all", qsTr("All")], ["videos", qsTr("Videos")], ["subtitles", qsTr("Subtitles")]]
                delegate: Button {
                    text: modelData[1]
                    flat: page.filter !== modelData[0]
                    highlighted: page.filter === modelData[0]
                    onClicked: page.filter = modelData[0]
                }
            }
            Item { Layout.fillWidth: true }
            TextField {
                id: searchField
                Layout.preferredWidth: 280
                placeholderText: qsTr("Search files and episodes")
                onTextChanged: page.query = text
                Keys.onEscapePressed: { text = ""; files.forceActiveFocus() }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: 24
            Layout.topMargin: 12
            radius: win.radius
            color: win.surface
            border.color: win.line
            clip: true

            ListView {
                id: files
                anchors.fill: parent
                anchors.margins: 1
                // A count model keeps the delegates (and the scroll position) across
                // the once-a-second refresh; each row reads its file by index.
                model: page.shown.length
                clip: true
                ScrollBar.vertical: ScrollBar {}
                delegate: ItemDelegate {
                    id: row
                    required property int index
                    readonly property var file: page.shown[index] || ({})
                    width: ListView.view.width
                    height: 56
                    onDoubleClicked: if (row.file.video) win.play(torrentKey, row.file.index)
                    contentItem: RowLayout {
                        spacing: 12
                        CheckBox {
                            checked: row.file.selected
                            ToolTip.text: checked ? qsTr("Downloading — uncheck to skip this file") : qsTr("Skipped — check to download")
                            ToolTip.visible: hovered
                            ToolTip.delay: 600
                            onToggled: library.setFileSelected(torrentKey, row.file.index, checked)
                        }
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 3
                            Label {
                                Layout.fillWidth: true
                                text: row.file.path
                                elide: Text.ElideMiddle
                                color: row.file.selected ? win.textColor : win.muted
                            }
                            RowLayout {
                                spacing: 10
                                ProgressBar {
                                    Layout.preferredWidth: 160
                                    from: 0; to: 1
                                    value: row.file.progress
                                    visible: row.file.selected && row.file.progress < 1
                                }
                                Label {
                                    color: win.muted
                                    font.pixelSize: 12
                                    text: {
                                        const parts = [fmt.bytes(row.file.length)]
                                        if (row.file.selected && row.file.progress < 1)
                                            parts.push(fmt.percent(row.file.progress))
                                        if (row.file.saved)
                                            parts.push(qsTr("Saved"))
                                        else if (!row.file.selected)
                                            parts.push(qsTr("Skipped"))
                                        if (row.file.positionMs > 0)
                                            parts.push(qsTr("Watched to %1").arg(fmt.clock(row.file.positionMs)))
                                        return parts.join(" · ")
                                    }
                                }
                            }
                        }
                        Button {
                            visible: row.file.video
                            text: row.file.positionMs > 0 ? qsTr("Resume") : qsTr("Play")
                            highlighted: row.file.index === info.defaultVideo
                            flat: row.file.index !== info.defaultVideo
                            onClicked: win.play(torrentKey, row.file.index)
                        }
                    }
                }
                Label {
                    anchors.centerIn: parent
                    visible: files.count === 0
                    text: (info.files || []).length === 0 ? qsTr("Finding files") : qsTr("No files match your search or filter.")
                    color: win.muted
                }
            }
        }
    }
}
