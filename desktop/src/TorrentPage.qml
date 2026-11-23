import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import TorrentPlayer

Page {
    id: page
    property string torrentKey
    // Re-read on every poll; the library bumps revision once a second.
    readonly property var info: library.revision >= 0 ? library.details(torrentKey) : ({})
    readonly property string pageTitle: info.name || ""
    property string filter: "all"
    property string query: ""
    readonly property var shown: visibleFiles()
    readonly property bool showing: StackView.status === StackView.Active
    background: Rectangle { color: "#0e0e10" }
    readonly property var engineHandle: engine

    // Layout: all actions in the top bar when there is room, else in a menu; the
    // content keeps to a readable width on a big screen.
    readonly property bool wideBar: width >= 820
    readonly property int bodyWidth: Math.min(width - 2 * win.pageMargin, 1280)
    readonly property int bodyX: Math.round((width - bodyWidth) / 2)
    readonly property bool searching: files.headerItem !== null && files.headerItem.searchFocused

    // The library dropped it (removed from another place): go back.
    onInfoChanged: if (info.key === undefined && StackView.status === StackView.Active) StackView.view.pop()

    Shortcut { sequence: "Space"; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen && !page.searching && info.defaultVideo >= 0; onActivated: win.play(torrentKey, info.defaultVideo) }
    Shortcut { sequence: "P"; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen && !page.searching; onActivated: library.togglePause(torrentKey) }
    Shortcut { sequence: "Delete"; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen && !page.searching; onActivated: win.confirmRemove(torrentKey) }
    Shortcut { sequences: ["/", "Ctrl+F"]; context: Qt.ApplicationShortcut; enabled: page.StackView.status === StackView.Active && !win.dialogOpen; onActivated: if (files.headerItem) files.headerItem.focusSearch() }

    function groupLabel(group) {
        if (group === "x") return qsTr("Extras")
        if (group.charAt(0) === "s") return qsTr("Season %1").arg(group.substring(1))
        return qsTr("Other files")
    }
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
            Layout.margins: win.compact ? 8 : 16
            Layout.bottomMargin: 0
            spacing: win.compact ? 4 : 8
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
                font.pixelSize: win.compact ? 17 : 20
                font.weight: Font.DemiBold
                color: win.textColor
            }
            Button {
                text: info.paused ? qsTr("Resume") : qsTr("Pause")
                flat: true
                visible: page.wideBar && info.state !== "done" && info.state !== "error"
                onClicked: library.togglePause(torrentKey)
            }
            // Direct Play streams only; this keeps the files instead (saved to Downloads).
            Button {
                text: qsTr("Download instead")
                flat: true
                visible: page.wideBar && info.directPlay === true
                ToolTip.text: qsTr("Save the files to %1").arg(library.saveDirectory)
                ToolTip.visible: hovered
                ToolTip.delay: 600
                onClicked: library.downloadInstead(torrentKey)
            }
            Button {
                text: qsTr("Retry")
                flat: true
                visible: page.wideBar && info.state === "error"
                onClicked: library.retry(torrentKey)
            }
            Button { text: qsTr("Open folder"); flat: true; visible: page.wideBar; onClicked: library.openFolder(torrentKey) }
            Button {
                text: qsTr("Peers")
                flat: true
                visible: page.wideBar
                onClicked: win.openPeers(torrentKey)
            }
            Button {
                text: qsTr("Remove…")
                flat: true
                visible: page.wideBar
                Material.foreground: win.danger
                onClicked: win.confirmRemove(torrentKey)
            }
            ToolButton {
                visible: !page.wideBar
                text: "⋮"
                font.pixelSize: 18
                ToolTip.text: qsTr("More")
                ToolTip.visible: hovered
                onClicked: actionsMenu.popup()
                Menu {
                    id: actionsMenu
                    Material.roundedScale: theme.radius
                    MenuItem {
                        text: info.paused ? qsTr("Resume") : qsTr("Pause")
                        visible: info.state !== "done" && info.state !== "error"
                        height: visible ? implicitHeight : 0
                        onTriggered: library.togglePause(torrentKey)
                    }
                    MenuItem {
                        text: qsTr("Download instead")
                        visible: info.directPlay === true
                        height: visible ? implicitHeight : 0
                        onTriggered: library.downloadInstead(torrentKey)
                    }
                    MenuItem {
                        text: qsTr("Retry")
                        visible: info.state === "error"
                        height: visible ? implicitHeight : 0
                        onTriggered: library.retry(torrentKey)
                    }
                    MenuItem { text: qsTr("Open folder"); onTriggered: library.openFolder(torrentKey) }
                    MenuItem { text: qsTr("Peers"); onTriggered: win.openPeers(torrentKey) }
                    MenuSeparator {}
                    MenuItem { text: qsTr("Remove…"); Material.foreground: win.danger; onTriggered: win.confirmRemove(torrentKey) }
                }
            }
        }

        // Summary and filters scroll away with the file list (the list's header),
        // so a short window still reaches every file.
        ListView {
            id: files
            objectName: "torrentFiles"
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.topMargin: 4
            // A count model keeps the delegates (and the scroll position) across
            // the once-a-second refresh; each row reads its file by index.
            model: page.shown.length
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {}
            footer: Item { width: files.width; height: win.pageMargin }
            property bool userScrolled: false
            onMovementStarted: userScrolled = true

            header: Item {
                id: headerBox
                width: files.width
                implicitHeight: headerColumn.implicitHeight + 12
                // The header grows as details arrive; stay at the top until the user scrolls.
                onHeightChanged: if (!files.userScrolled) files.positionViewAtBeginning()
                readonly property bool searchFocused: searchField.activeFocus
                function focusSearch() {
                    files.userScrolled = false
                    files.positionViewAtBeginning()
                    searchField.forceActiveFocus()
                }

                ColumnLayout {
                    id: headerColumn
                    x: page.bodyX
                    width: page.bodyWidth
                    spacing: 0

                    // Summary: picture, play, and the numbers. One column on a narrow window.
                    GridLayout {
                        id: summaryGrid
                        readonly property bool stacked: page.bodyWidth < 640
                        Layout.fillWidth: true
                        Layout.topMargin: win.compact ? 8 : 24
                        Layout.bottomMargin: win.compact ? 12 : 24
                        columns: stacked ? 1 : 2
                        columnSpacing: 24
                        rowSpacing: 16

                        Rectangle {
                            Layout.alignment: Qt.AlignTop
                            // Stacked, it is as wide as the column, but no taller than about half a short window.
                            readonly property int pictureWidth: summaryGrid.stacked
                                ? Math.max(160, Math.min(page.bodyWidth, 480, Math.round(page.height * 0.45 * 16 / 9)))
                                : 320
                            Layout.preferredWidth: pictureWidth
                            Layout.preferredHeight: Math.round(pictureWidth * 9 / 16)
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
                            id: details
                            Layout.fillWidth: true
                            Layout.alignment: Qt.AlignTop
                            spacing: 10
                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 12
                                Label {
                                    text: win.stateLabel(info.state)
                                    color: info.state === "error" ? win.danger : theme.accent
                                    font.pixelSize: 15
                                    font.weight: Font.Medium
                                }
                                Rectangle {
                                    visible: info.directPlay === true
                                    implicitWidth: directPlayBadge.implicitWidth + 14
                                    implicitHeight: directPlayBadge.implicitHeight + 6
                                    radius: win.smallRadius
                                    color: "transparent"
                                    border.color: theme.accent
                                    Label {
                                        id: directPlayBadge
                                        anchors.centerIn: parent
                                        text: qsTr("Direct Play")
                                        font.pixelSize: 11
                                        color: theme.accent
                                    }
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
                                Layout.fillWidth: true
                                columns: details.width < 440 ? 2 : 4
                                columnSpacing: details.width < 440 ? 16 : 32
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
                                        Layout.maximumWidth: 200
                                        Label { Layout.fillWidth: true; text: modelData[0]; color: win.muted; font.pixelSize: 12; elide: Text.ElideRight }
                                        Label { Layout.fillWidth: true; text: modelData[1]; color: win.textColor; font.pixelSize: 14; elide: Text.ElideRight }
                                    }
                                }
                            }
                            // Info hash and its copy buttons wrap under each other when narrow.
                            Flow {
                                Layout.fillWidth: true
                                spacing: 4
                                Row {
                                    spacing: 4
                                    height: copyHash.height
                                    Label {
                                        id: hashTitle
                                        anchors.verticalCenter: parent.verticalCenter
                                        text: qsTr("Info hash")
                                        color: win.muted
                                        font.pixelSize: 12
                                    }
                                    Label {
                                        anchors.verticalCenter: parent.verticalCenter
                                        width: Math.min(implicitWidth, 380, details.width - hashTitle.width - 8)
                                        text: torrentKey
                                        color: win.muted
                                        font.family: "monospace"
                                        font.pixelSize: 12
                                        elide: Text.ElideMiddle
                                    }
                                }
                                ToolButton {
                                    id: copyHash
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

                            // Pieces map (Android PieceHeatGridSection), under the numbers.
                            RowLayout {
                                Layout.fillWidth: true
                                Layout.maximumWidth: 560
                                Layout.topMargin: 4
                                spacing: 8
                                Label {
                                    text: info.directPlay ? qsTr("RAM pieces") : qsTr("Pieces")
                                    color: win.textColor
                                    font.pixelSize: 14
                                    font.weight: Font.DemiBold
                                }
                                Label {
                                    Layout.fillWidth: true
                                    horizontalAlignment: Text.AlignRight
                                    visible: pieceGrid.hasMap
                                    text: pieceGrid.summary.titleMeta || ""
                                    color: win.muted
                                    font.pixelSize: 11
                                    elide: Text.ElideRight
                                }
                                Item { Layout.fillWidth: true; visible: !pieceGrid.hasMap }
                                Rectangle {
                                    visible: pieceGrid.hasMap
                                    implicitWidth: percentPill.implicitWidth + 16
                                    implicitHeight: percentPill.implicitHeight + 4
                                    radius: height / 2
                                    color: Qt.alpha(theme.accent, 0.22)
                                    Label {
                                        id: percentPill
                                        anchors.centerIn: parent
                                        text: pieceGrid.summary.verifiedPercent >= 0 ? pieceGrid.summary.verifiedPercent + "%" : "—"
                                        color: win.textColor
                                        font.pixelSize: 12
                                        font.weight: Font.Bold
                                    }
                                }
                            }
                            Rectangle {
                                Layout.fillWidth: true
                                Layout.maximumWidth: 560
                                implicitHeight: pieceGrid.hasMap ? pieceGrid.implicitHeight + 16 : unavailable.implicitHeight + 20
                                radius: win.radius
                                color: Qt.rgba(0.16, 0.16, 0.18, 0.55)
                                PieceGrid {
                                    id: pieceGrid
                                    anchors { left: parent.left; right: parent.right; top: parent.top; margins: 8 }
                                    height: implicitHeight
                                    engine: page.engineHandle
                                    torrentId: info.engineId || ""
                                    active: page.showing
                                    frozen: info.paused === true || (info.progress || 0) >= 1
                                    memoryBuffer: info.directPlay === true
                                    files: info.files || []
                                    doneColor: theme.accent
                                    outlineColor: win.muted
                                    onSurfaceColor: win.textColor
                                    Accessible.name: summary.accessible || ""
                                }
                                Column {
                                    id: unavailable
                                    visible: !pieceGrid.hasMap
                                    anchors { left: parent.left; right: parent.right; top: parent.top; margins: 10 }
                                    Label { width: parent.width; text: qsTr("Map unavailable"); color: win.textColor; font.pixelSize: 13; wrapMode: Text.Wrap }
                                    Label { width: parent.width; text: qsTr("Engine offline or telemetry not ready yet"); color: win.muted; font.pixelSize: 11; wrapMode: Text.Wrap }
                                }
                            }
                            Flow {
                                Layout.fillWidth: true
                                visible: pieceGrid.hasMap
                                spacing: 14
                                Repeater {
                                    model: [
                                        [theme.accent, false, info.directPlay ? qsTr("Buffered") : qsTr("Done"), pieceGrid.summary.done],
                                        [pieceGrid.gettingColor, false, qsTr("Getting"), pieceGrid.summary.getting],
                                        [win.muted, true, info.directPlay ? qsTr("Empty") : qsTr("Waiting"), pieceGrid.summary.waiting],
                                        [win.muted, true, qsTr("Off"), pieceGrid.summary.off]
                                    ]
                                    delegate: Row {
                                        visible: index < 3 || modelData[3] > 0
                                        spacing: 5
                                        Rectangle {
                                            anchors.verticalCenter: parent.verticalCenter
                                            width: 8; height: 8; radius: 4
                                            color: modelData[1] ? "transparent" : modelData[0]
                                            border.color: modelData[0]
                                            border.width: modelData[1] ? 1.25 : 0
                                        }
                                        Label { text: modelData[2] + " " + (modelData[3] || 0); color: win.muted; font.pixelSize: 11 }
                                    }
                                }
                                Label {
                                    visible: text !== ""
                                    width: Math.min(implicitWidth, details.width)
                                    text: pieceGrid.summary.bufferLine || ""
                                    color: win.muted
                                    font.pixelSize: 11
                                    elide: Text.ElideRight
                                }
                            }
                            Label {
                                Layout.fillWidth: true
                                Layout.maximumWidth: 560
                                visible: pieceGrid.hasMap
                                text: pieceGrid.peekText !== "" ? pieceGrid.peekText : pieceGrid.summary.titlePrimary || ""
                                color: win.muted
                                font.pixelSize: 11
                                elide: Text.ElideRight
                            }
                        }
                    }

                    // Files: filters, then search (on a row of its own when narrow).
                    GridLayout {
                        Layout.fillWidth: true
                        Layout.bottomMargin: 12
                        columns: page.bodyWidth < 560 ? 1 : 2
                        columnSpacing: 8
                        rowSpacing: 4
                        RowLayout {
                            spacing: 8
                            Layout.fillWidth: true
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
                        }
                        TextField {
                            id: searchField
                            Layout.fillWidth: page.bodyWidth < 560
                            Layout.preferredWidth: page.bodyWidth < 560 ? -1 : 280
                            placeholderText: qsTr("Search files and episodes")
                            onTextChanged: page.query = text
                            Keys.onEscapePressed: { text = ""; files.forceActiveFocus() }
                        }
                    }
                }
            }

            // ListView places its delegates at x 0, so the row is centred inside a full-width item.
            delegate: Item {
                id: rowSlot
                required property int index
                width: files.width
                height: rowBox.height
                Column {
                    id: rowBox
                    readonly property int index: rowSlot.index
                    readonly property var file: page.shown[index] || ({})
                    // Season headers, as Android's natural order groups episodes; extras last.
                    readonly property bool header: info.grouped === true && file.group !== undefined &&
                                                   (index === 0 || (page.shown[index - 1] || {}).group !== file.group)
                    x: page.bodyX
                    width: page.bodyWidth
                    Label {
                        visible: rowBox.header
                        height: visible ? 34 : 0
                        leftPadding: 16
                        verticalAlignment: Text.AlignBottom
                        bottomPadding: 4
                        text: page.groupLabel(rowBox.file.group || "")
                        color: theme.accent
                        font.pixelSize: 13
                        font.weight: Font.DemiBold
                    }
                    ItemDelegate {
                        id: row
                        readonly property int index: rowBox.index
                        readonly property var file: rowBox.file
                        width: rowBox.width
                        height: 56
                        leftPadding: win.compact ? 4 : 12
                        rightPadding: win.compact ? 4 : 12
                        onDoubleClicked: if (row.file.video) win.play(torrentKey, row.file.index)
                        background: Rectangle {
                            color: row.hovered ? win.surfaceHigh : win.surface
                            Rectangle { anchors { left: parent.left; right: parent.right; bottom: parent.bottom } height: 1; color: win.line }
                        }
                        contentItem: RowLayout {
                            spacing: win.compact ? 4 : 12
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
                                    text: info.grouped && row.file.video ? row.file.episode + "  —  " + row.file.path : row.file.path
                                    elide: Text.ElideMiddle
                                    color: row.file.selected ? win.textColor : win.muted
                                }
                                RowLayout {
                                    Layout.fillWidth: true
                                    spacing: 10
                                    ProgressBar {
                                        Layout.preferredWidth: Math.min(160, page.bodyWidth / 5)
                                        from: 0; to: 1
                                        value: row.file.progress
                                        visible: row.file.selected && row.file.progress < 1
                                    }
                                    Label {
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                        color: win.muted
                                        font.pixelSize: 12
                                        text: {
                                            const parts = [fmt.bytes(row.file.length)]
                                            if (row.file.upNext)
                                                parts.push(row.file.upNext)
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
                }
            }

            Label {
                // Children of a ListView live in its content item, so this scrolls with the header.
                x: (files.width - width) / 2
                y: files.headerItem ? files.headerItem.y + files.headerItem.height + 24 : 24
                width: Math.min(implicitWidth, page.bodyWidth)
                wrapMode: Text.Wrap
                horizontalAlignment: Text.AlignHCenter
                visible: files.count === 0
                text: (info.files || []).length === 0 ? qsTr("Finding files") : qsTr("No files match your search or filter.")
                color: win.muted
            }
        }
    }
}
