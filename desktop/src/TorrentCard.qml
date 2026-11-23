import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Item {
    id: card
    property bool current: false
    // The 16:9 picture's height; the library trims it on a short window.
    property int pictureHeight: Math.round((width - 18) * 9 / 16)
    readonly property string cardKey: model.key
    signal clicked()

    Rectangle {
        id: body
        anchors.fill: parent
        anchors.margins: 8
        radius: win.radius
        color: hover.hovered ? win.surfaceHigh : win.surface
        border.color: card.current ? theme.accent : win.line
        border.width: card.current ? 2 : 1
        clip: true

        // 16:9 picture: the poster or a frame from the video.
        Rectangle {
            id: picture
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 1 }
            height: card.pictureHeight
            color: "#111114"
            radius: Math.max(0, win.radius - 1)
            clip: true

            Image {
                anchors.fill: parent
                source: model.thumbnail
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                visible: status === Image.Ready
                sourceSize.width: 640
            }
            Label {
                anchors.centerIn: parent
                visible: model.thumbnail === ""
                text: (model.name || "?").charAt(0).toUpperCase()
                font.pixelSize: Math.max(12, picture.height * 0.36)
                font.weight: Font.Bold
                color: "#2c2c33"
            }
            // Play over the picture, for torrents with a video.
            RoundButton {
                anchors.centerIn: parent
                visible: model.hasVideo && (hover.hovered || card.current)
                width: Math.min(64, picture.height - 8)
                height: width
                text: "▶"
                font.pixelSize: 24
                Material.background: theme.accent
                Material.foreground: theme.accentForeground
                onClicked: win.play(model.key)
            }
            // Direct Play streams only: nothing is saved to Downloads.
            Rectangle {
                anchors { left: parent.left; top: parent.top; margins: 8 }
                visible: model.directPlay
                width: directPlayText.implicitWidth + 14
                height: directPlayText.implicitHeight + 6
                radius: win.smallRadius
                color: Qt.rgba(0, 0, 0, 0.7)
                border.color: theme.accent
                Label {
                    id: directPlayText
                    anchors.centerIn: parent
                    text: qsTr("Direct Play")
                    font.pixelSize: 11
                    color: theme.accent
                }
            }
            Rectangle {
                anchors { right: parent.right; top: parent.top; margins: 8 }
                visible: model.state !== "downloading"
                width: stateText.implicitWidth + 14
                height: stateText.implicitHeight + 6
                radius: win.smallRadius
                color: model.state === "error" ? win.danger : model.state === "done" ? theme.accent : Qt.rgba(0, 0, 0, 0.7)
                Label {
                    id: stateText
                    anchors.centerIn: parent
                    text: win.stateLabel(model.state)
                    font.pixelSize: 11
                    color: model.state === "done" ? theme.accentForeground : (model.state === "error" ? "white" : win.textColor)
                }
            }
        }

        ColumnLayout {
            anchors { left: parent.left; right: parent.right; top: picture.bottom; margins: 12 }
            spacing: 6

            Label {
                Layout.fillWidth: true
                text: model.name
                elide: Text.ElideRight
                color: win.textColor
                font.pixelSize: 14
                font.weight: Font.Medium
                ToolTip.text: model.name
                ToolTip.visible: hover.hovered && truncated
                ToolTip.delay: 600
            }
            ProgressBar {
                Layout.fillWidth: true
                from: 0
                to: 1
                value: model.progress
                visible: model.state !== "done"
                indeterminate: model.state === "fetching" || model.state === "starting"
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    color: model.state === "error" ? win.danger : win.muted
                    font.pixelSize: 12
                    text: {
                        if (model.state === "error")
                            return model.error
                        if (model.state === "done")
                            return fmt.bytes(model.size)
                        const parts = [fmt.percent(model.progress) + " · " + fmt.bytes(model.downloaded) + " / " + fmt.bytes(model.size)]
                        if (model.state === "downloading") {
                            parts.push(fmt.speed(model.downloadSpeed))
                            if (fmt.eta(model.eta) !== "")
                                parts.push(fmt.eta(model.eta))
                        } else if (model.state !== "paused") {
                            parts.push(qsTr("%n peers", "", model.peers))
                        }
                        return parts.join(" · ")
                    }
                }
                ToolButton {
                    visible: model.state !== "done" && (hover.hovered || card.current)
                    text: model.state === "paused" ? "▶" : "❚❚"
                    font.pixelSize: 11
                    implicitWidth: 32
                    implicitHeight: 28
                    ToolTip.text: model.state === "paused" ? qsTr("Resume") : qsTr("Pause")
                    ToolTip.visible: hovered
                    onClicked: model.state === "error" ? library.retry(model.key) : library.togglePause(model.key)
                }
            }
        }

        HoverHandler { id: hover }
        // A click opens the torrent, like a tap on Android.
        TapHandler {
            onTapped: { card.clicked(); win.openTorrent(model.key) }
        }
        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: { card.clicked(); menu.popup() }
        }
        Menu {
            id: menu
            Material.roundedScale: theme.radius
            MenuItem { text: qsTr("Play"); enabled: model.hasVideo; onTriggered: win.play(model.key) }
            MenuItem { text: qsTr("Details"); onTriggered: win.openTorrent(model.key) }
            MenuItem { text: model.state === "paused" ? qsTr("Resume") : qsTr("Pause"); enabled: model.state !== "done"; onTriggered: library.togglePause(model.key) }
            MenuItem { text: qsTr("Retry"); visible: model.state === "error"; height: visible ? implicitHeight : 0; onTriggered: library.retry(model.key) }
            MenuItem { text: qsTr("Open folder"); onTriggered: library.openFolder(model.key) }
            MenuItem { text: qsTr("Copy magnet link"); onTriggered: { library.copyMagnet(model.key); win.toast(qsTr("Magnet link copied")) } }
            MenuSeparator {}
            MenuItem { text: qsTr("Remove…"); onTriggered: win.confirmRemove(model.key) }
        }
    }

    MouseArea {
        anchors.fill: body
        acceptedButtons: Qt.NoButton
        cursorShape: Qt.PointingHandCursor
    }
}
