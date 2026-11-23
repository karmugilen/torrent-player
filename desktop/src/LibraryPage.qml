import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Page {
    id: page
    readonly property string pageTitle: ""
    background: Rectangle { color: "#0e0e10" }

    // Keyboard actions on the selected card.
    readonly property string currentKey: grid.currentItem ? grid.currentItem.cardKey : ""
    Shortcut { sequence: "Space"; context: Qt.ApplicationShortcut; enabled: win.onLibrary && page.currentKey !== "" && !search.activeFocus; onActivated: win.play(page.currentKey) }
    Shortcut { sequences: ["Return", "Enter"]; context: Qt.ApplicationShortcut; enabled: win.onLibrary && page.currentKey !== "" && !search.activeFocus; onActivated: win.openTorrent(page.currentKey) }
    Shortcut { sequence: "P"; context: Qt.ApplicationShortcut; enabled: win.onLibrary && page.currentKey !== "" && !search.activeFocus; onActivated: library.togglePause(page.currentKey) }
    Shortcut { sequence: "Delete"; context: Qt.ApplicationShortcut; enabled: win.onLibrary && page.currentKey !== "" && !search.activeFocus; onActivated: win.confirmRemove(page.currentKey) }
    Shortcut { sequences: ["/", "Ctrl+F"]; context: Qt.ApplicationShortcut; enabled: win.onLibrary; onActivated: search.forceActiveFocus() }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Header: name, search, totals, and the ways in.
        RowLayout {
            Layout.fillWidth: true
            Layout.margins: 24
            Layout.bottomMargin: 12
            spacing: 16

            Label {
                text: "Torrent Player"
                font.pixelSize: 24
                font.weight: Font.DemiBold
                color: win.textColor
            }
            Item { Layout.fillWidth: true }
            Label {
                visible: library.downloadSpeed > 0 || library.uploadSpeed > 0
                text: "↓ " + fmt.speed(library.downloadSpeed) + "   ↑ " + fmt.speed(library.uploadSpeed)
                color: win.muted
                font.pixelSize: 13
            }
            TextField {
                id: search
                Layout.preferredWidth: 260
                placeholderText: qsTr("Search downloads")
                visible: library.count > 0
                onTextChanged: libraryView.setFilterFixedString(text)
                Keys.onEscapePressed: { text = ""; grid.forceActiveFocus() }
                Keys.onDownPressed: grid.forceActiveFocus()
            }
            Button {
                text: qsTr("Open file")
                flat: true
                onClicked: picker.open("torrent")
            }
            Button {
                text: qsTr("Add")
                highlighted: true
                onClicked: win.openAdd("")
            }
            ToolButton {
                text: "⚙"
                font.pixelSize: 18
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Settings")
                onClicked: win.openSettings()
            }
        }

        GridView {
            id: grid
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            clip: true
            focus: true
            model: libraryView
            keyNavigationEnabled: true
            readonly property int columns: Math.max(1, Math.floor(width / 300))
            cellWidth: Math.floor(width / columns)
            cellHeight: cellWidth * 9 / 16 + 96
            ScrollBar.vertical: ScrollBar {}
            delegate: TorrentCard {
                width: grid.cellWidth
                height: grid.cellHeight
                current: GridView.isCurrentItem && grid.activeFocus
                onClicked: { grid.currentIndex = index; grid.forceActiveFocus() }
            }
            Component.onCompleted: forceActiveFocus()
        }
    }

    // Nothing yet, or nothing that matches.
    ColumnLayout {
        anchors.centerIn: parent
        width: Math.min(440, parent.width - 64)
        visible: grid.count === 0
        spacing: 12
        Label {
            Layout.alignment: Qt.AlignHCenter
            text: library.count === 0 ? qsTr("No downloads yet") : qsTr("No matching downloads")
            font.pixelSize: 20
            color: win.textColor
        }
        Label {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            color: win.muted
            text: library.count === 0
                  ? qsTr("Add a magnet link, a link to a .torrent file, or a .torrent file from your computer. You can also drop one here.")
                  : qsTr("No items match '%1'. Try another search term.").arg(search.text)
        }
        RowLayout {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 8
            visible: library.count === 0
            spacing: 12
            Button { text: qsTr("Paste a link"); highlighted: true; onClicked: win.openAdd(library.clipboardText()) }
            Button { text: qsTr("Open torrent file"); flat: true; onClicked: picker.open("torrent") }
        }
        Label {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: 8
            visible: library.count === 0
            text: qsTr("Press ? for keyboard shortcuts")
            color: win.muted
            font.pixelSize: 12
        }
    }
}
