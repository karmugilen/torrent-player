import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    color: Qt.rgba(0, 0, 0, 0.82)

    readonly property var sections: [
        [qsTr("Everywhere"), [
            ["?", qsTr("Show or hide shortcuts")],
            ["A", qsTr("Add a magnet link or torrent URL")],
            ["Ctrl+V", qsTr("Add the link on the clipboard")],
            ["O", qsTr("Open a .torrent file")],
            [",", qsTr("Settings")],
            ["Esc", qsTr("Back")],
            ["Q", qsTr("Quit")]]],
        [qsTr("Library and torrent"), [
            [qsTr("Arrows"), qsTr("Move between downloads")],
            ["Enter", qsTr("Open the download")],
            ["Space", qsTr("Play")],
            ["P", qsTr("Pause or resume")],
            ["Delete", qsTr("Remove")],
            ["/", qsTr("Search")]]],
        [qsTr("Player"), [
            ["Space", qsTr("Play or pause")],
            ["← →", qsTr("Back or forward 10 seconds")],
            [qsTr("Hold →"), qsTr("Play at 2x while held")],
            ["Shift+← →", qsTr("Back or forward 1 minute")],
            ["↑ ↓", qsTr("Volume")],
            ["M", qsTr("Mute")],
            ["F", qsTr("Full screen (or double-click)")],
            ["P", qsTr("Picture-in-picture")],
            ["C", qsTr("Next subtitles")],
            ["N", qsTr("Next episode")],
            ["Shift+N", qsTr("Previous episode")],
            ["I", qsTr("Show or hide stats and pieces")],
            [".", qsTr("Next frame")],
            ["V", qsTr("Open in mpv")],
            ["Esc", qsTr("Back")]]]
    ]

    Flickable {
        id: flick
        anchors.fill: parent
        contentWidth: Math.max(width, helpBody.implicitWidth + 48)
        contentHeight: Math.max(height, helpBody.implicitHeight + 48)
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar { policy: flick.contentHeight > flick.height ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff }
        ScrollBar.horizontal: ScrollBar { policy: flick.contentWidth > flick.width ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff }

        MouseArea {
            width: Math.max(flick.width, helpBody.implicitWidth + 48)
            height: Math.max(flick.height, helpBody.implicitHeight + 48)
            onClicked: win.helpVisible = false
        }

        RowLayout {
            id: helpBody
            x: Math.max(24, (flick.width - implicitWidth) / 2)
            y: Math.max(24, (flick.height - implicitHeight) / 2)
            spacing: win.compact ? 28 : 56
            Repeater {
                model: sections
                delegate: ColumnLayout {
                    Layout.alignment: Qt.AlignTop
                    spacing: 10
                    Label { text: modelData[0]; color: theme.accent; font.pixelSize: 16; font.weight: Font.DemiBold }
                    Repeater {
                        model: modelData[1]
                        delegate: RowLayout {
                            spacing: 16
                            Rectangle {
                                Layout.preferredWidth: 96
                                Layout.preferredHeight: 26
                                radius: win.smallRadius
                                color: "#26262b"
                                Label { anchors.centerIn: parent; text: modelData[0]; color: win.textColor; font.pixelSize: 12; font.family: "monospace" }
                            }
                            Label { text: modelData[1]; color: win.textColor; font.pixelSize: 14 }
                        }
                    }
                }
            }
        }
    }
}
