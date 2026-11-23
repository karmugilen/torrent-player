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
    Shortcut { sequences: ["/", "Ctrl+F"]; context: Qt.ApplicationShortcut; enabled: win.onLibrary; onActivated: page.openSearch() }

    // On a narrow window search hides behind an icon and opens as its own row.
    property bool searchOpen: false
    readonly property bool searchShown: library.count > 0 && (!win.compact || searchOpen || search.text !== "")
    function openSearch() {
        searchOpen = true
        search.forceActiveFocus()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Header: name, search, totals, and the ways in. Narrow, the search field and
        // the speeds move to rows of their own and the rarer actions to a menu.
        GridLayout {
            id: header
            readonly property bool narrow: win.compact
            Layout.fillWidth: true
            Layout.margins: win.pageMargin
            Layout.bottomMargin: narrow ? 8 : 12
            columns: narrow ? 4 : 7
            columnSpacing: narrow ? 4 : 16
            rowSpacing: 4

            Label {
                Layout.row: 0; Layout.column: 0
                Layout.fillWidth: true
                text: "Torrent Player"
                elide: Text.ElideRight
                font.pixelSize: header.narrow ? 20 : 24
                font.weight: Font.DemiBold
                color: win.textColor
            }
            Label {
                Layout.row: header.narrow ? 2 : 0
                Layout.column: header.narrow ? 0 : 1
                Layout.columnSpan: header.narrow ? 4 : 1
                visible: library.downloadSpeed > 0 || library.uploadSpeed > 0
                text: "↓ " + fmt.speed(library.downloadSpeed) + "   ↑ " + fmt.speed(library.uploadSpeed)
                color: win.muted
                font.pixelSize: header.narrow ? 12 : 13
            }
            TextField {
                id: search
                Layout.row: header.narrow ? 1 : 0
                Layout.column: header.narrow ? 0 : 2
                Layout.columnSpan: header.narrow ? 4 : 1
                Layout.fillWidth: header.narrow
                Layout.preferredWidth: header.narrow ? -1 : 260
                Layout.minimumWidth: header.narrow ? 0 : 160
                visible: page.searchShown
                placeholderText: qsTr("Search downloads")
                onTextChanged: libraryView.setFilterFixedString(text)
                Keys.onEscapePressed: { text = ""; page.searchOpen = false; grid.forceActiveFocus() }
                Keys.onDownPressed: grid.forceActiveFocus()
            }
            ToolButton {
                Layout.row: 0; Layout.column: header.narrow ? 1 : 3
                visible: header.narrow && library.count > 0
                text: "⌕"
                font.pixelSize: 22
                Accessible.name: qsTr("Search downloads")
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Search downloads")
                onClicked: {
                    if (page.searchOpen) {
                        search.text = ""
                        page.searchOpen = false
                        grid.forceActiveFocus()
                    } else {
                        page.openSearch()
                    }
                }
            }
            Button {
                Layout.row: 0; Layout.column: 4
                visible: !header.narrow
                text: qsTr("Open file")
                flat: true
                onClicked: picker.open("torrent")
            }
            Button {
                Layout.row: 0; Layout.column: header.narrow ? 2 : 5
                text: header.narrow ? "+" : qsTr("Add")
                font.pixelSize: header.narrow ? 20 : 14
                highlighted: true
                Accessible.name: qsTr("Add")
                ToolTip.visible: header.narrow && hovered
                ToolTip.text: qsTr("Add")
                onClicked: win.openAdd("")
            }
            ToolButton {
                Layout.row: 0; Layout.column: header.narrow ? 3 : 6
                text: header.narrow ? "⋮" : "⚙"
                font.pixelSize: 18
                ToolTip.visible: hovered
                ToolTip.text: header.narrow ? qsTr("More") : qsTr("Settings")
                onClicked: header.narrow ? moreMenu.popup() : win.openSettings()
                Menu {
                    id: moreMenu
                    Material.roundedScale: theme.radius
                    MenuItem { text: qsTr("Open file"); onTriggered: picker.open("torrent") }
                    MenuItem { text: qsTr("Settings"); onTriggered: win.openSettings() }
                }
            }
        }

        GridView {
            id: grid
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.leftMargin: win.pageMargin - 8
            Layout.rightMargin: win.pageMargin - 8
            clip: true
            focus: true
            model: libraryView
            keyNavigationEnabled: true
            // One column on a narrow window; on a huge one, cards stay near 300-420 px.
            readonly property int columns: Math.max(1, Math.floor(width / 300))
            cellWidth: Math.floor(width / columns)
            // A short window trims the picture so a whole card still fits.
            readonly property int pictureHeight: Math.max(72, Math.min(Math.round((cellWidth - 18) * 9 / 16), height - 106))
            cellHeight: pictureHeight + 106
            ScrollBar.vertical: ScrollBar {}
            delegate: TorrentCard {
                pictureHeight: grid.pictureHeight
                width: grid.cellWidth
                height: grid.cellHeight
                current: GridView.isCurrentItem && grid.activeFocus
                onClicked: { grid.currentIndex = index; grid.forceActiveFocus() }
            }
            Component.onCompleted: forceActiveFocus()
        }
    }

    // Nothing yet, or nothing that matches: centred under the header, scrolling when short.
    Flickable {
        id: emptyFlick
        x: grid.x
        y: grid.y
        width: grid.width
        height: grid.height
        visible: grid.count === 0
        contentWidth: width
        contentHeight: Math.max(height, empty.implicitHeight + 32)
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar {}

        ColumnLayout {
            id: empty
            width: Math.min(440, emptyFlick.width - 32)
            x: (emptyFlick.width - width) / 2
            y: Math.max(16, (emptyFlick.contentHeight - implicitHeight) / 2)
            spacing: 12
            Label {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
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
            Flow {
                Layout.alignment: Qt.AlignHCenter
                Layout.maximumWidth: empty.width
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
}
