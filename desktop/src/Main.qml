import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

ApplicationWindow {
    id: win
    width: 1180
    height: 760
    minimumWidth: 720
    minimumHeight: 480
    visible: true
    title: stack.currentItem && stack.currentItem.pageTitle ? stack.currentItem.pageTitle + " — Torrent Player" : "Torrent Player"
    color: "#0e0e10"

    Material.theme: Material.Dark
    Material.accent: theme.accent
    Material.primary: theme.accent
    Material.background: "#0e0e10"
    Material.roundedScale: theme.radius

    readonly property color surface: "#17171a"
    readonly property color surfaceHigh: "#202024"
    readonly property color line: "#2a2a2f"
    readonly property color textColor: "#ececef"
    readonly property color muted: "#9d9da6"
    readonly property color danger: "#ff6b6b"
    readonly property int radius: theme.radius
    readonly property int smallRadius: Math.min(4, theme.radius)

    property bool helpVisible: false
    property bool quitConfirmed: false
    readonly property bool dialogOpen: addDialog.visible || deleteDialog.visible || quitDialog.visible || helpVisible
    readonly property bool onLibrary: stack.depth === 1 && !dialogOpen
    readonly property bool onPlayer: stack.currentItem && stack.currentItem.isPlayer === true

    function stateLabel(state) {
        switch (state) {
        case "error": return qsTr("Error")
        case "paused": return qsTr("Paused")
        case "starting": return qsTr("Starting")
        case "fetching": return qsTr("Finding files")
        case "checking": return qsTr("Checking files")
        case "done": return qsTr("Done")
        case "downloading": return qsTr("Downloading")
        case "searching": return qsTr("Looking for peers")
        default: return qsTr("Waiting for peers")
        }
    }

    function openAdd(text) {
        addDialog.openWith(text || "")
    }
    function openTorrent(key) {
        if (stack.currentItem && stack.currentItem.torrentKey === key && !onPlayer)
            return
        stack.push(torrentPage, { torrentKey: key })
    }
    function goHome() {
        stack.pop(null)
    }
    function confirmRemove(key) {
        deleteDialog.ask(key)
    }
    function play(key, fileIndex) {
        library.play(key, fileIndex === undefined ? -1 : fileIndex)
    }
    function toast(text) {
        toastLabel.text = text
        toastBox.opacity = 1
        toastTimer.restart()
    }
    function requestQuit() {
        if (library.activeCount > 0 && !quitConfirmed)
            quitDialog.open()
        else {
            quitConfirmed = true
            Qt.quit()
        }
    }

    onClosing: (close) => {
        if (library.activeCount > 0 && !quitConfirmed) {
            close.accepted = false
            quitDialog.open()
        }
    }

    Connections {
        target: library
        function onPlayReady(media) {
            if (onPlayer)
                stack.pop()
            stack.push(playerPage, { media: media })
        }
        function onMessage(text) { win.toast(text) }
    }
    Connections {
        target: adder
        function onStateChanged() {
            if (adder.state !== "idle" && !addDialog.visible)
                addDialog.open()
        }
        function onAdded(key, playNow) {
            addDialog.close()
            if (playNow)
                win.play(key, -1)
            else
                win.toast(qsTr("Added to your library"))
        }
    }
    Connections {
        target: picker
        function onPicked(kind, path) {
            if (kind === "torrent")
                win.openAdd(path)
            else if (onPlayer)
                stack.currentItem.loadSubtitleFile(path)
        }
        function onFailed(message) { win.toast(message) }
    }

    Shortcut { sequence: "?"; context: Qt.ApplicationShortcut; enabled: !addDialog.visible; onActivated: win.helpVisible = !win.helpVisible }
    Shortcut { sequence: "Q"; context: Qt.ApplicationShortcut; enabled: !dialogOpen; onActivated: win.requestQuit() }
    Shortcut { sequence: "Ctrl+Q"; context: Qt.ApplicationShortcut; onActivated: win.requestQuit() }
    Shortcut { sequence: "A"; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: win.openAdd("") }
    Shortcut { sequences: [StandardKey.Paste]; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer && !(activeFocusItem instanceof TextInput); onActivated: win.openAdd(library.clipboardText()) }
    Shortcut { sequence: "O"; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: picker.open("torrent") }
    Shortcut { sequence: "Ctrl+O"; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: picker.open("torrent") }
    Shortcut {
        sequences: ["Esc", "Backspace", "Alt+Left"]
        context: Qt.ApplicationShortcut
        enabled: !dialogOpen && stack.depth > 1 && !(activeFocusItem instanceof TextInput)
        onActivated: {
            if (onPlayer && win.visibility === Window.FullScreen)
                win.showNormal()
            else
                stack.pop()
        }
    }
    Shortcut { sequence: "Esc"; context: Qt.ApplicationShortcut; enabled: helpVisible; onActivated: helpVisible = false }

    Item {
        anchors.fill: parent
        LayoutMirroring.enabled: Qt.application.layoutDirection === Qt.RightToLeft
        LayoutMirroring.childrenInherit: true

        StackView {
            id: stack
            anchors.fill: parent
            initialItem: LibraryPage {}
        }

        // Drop a .torrent file or a magnet link anywhere to add it.
        DropArea {
            id: drop
            anchors.fill: parent
            enabled: !onPlayer
            keys: ["text/uri-list", "text/plain"]
            onDropped: (event) => {
                const text = event.hasUrls ? event.urls[0].toString() : event.text
                win.openAdd(text)
                event.acceptProposedAction()
            }
            Rectangle {
                anchors.fill: parent
                anchors.margins: 12
                visible: drop.containsDrag
                color: Qt.rgba(0, 0, 0, 0.6)
                border.color: theme.accent
                border.width: 2
                radius: win.radius
                Label {
                    anchors.centerIn: parent
                    text: qsTr("Drop to add the torrent")
                    font.pixelSize: 22
                    color: win.textColor
                }
            }
        }

        // The engine is the app; when it isn't running, say so up front.
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: engineBanner.implicitHeight + 24
            visible: engine.error !== "" && !onPlayer
            color: "#2a1215"
            RowLayout {
                id: engineBanner
                anchors { fill: parent; margins: 12; leftMargin: 20; rightMargin: 20 }
                Label {
                    Layout.fillWidth: true
                    text: engine.error
                    color: win.textColor
                    wrapMode: Text.Wrap
                }
                Button { text: qsTr("Restart engine"); flat: true; onClicked: engine.restart() }
            }
        }

        Rectangle {
            id: toastBox
            anchors { horizontalCenter: parent.horizontalCenter; bottom: parent.bottom; bottomMargin: 32 }
            width: Math.min(toastLabel.implicitWidth + 40, parent.width - 64)
            height: toastLabel.implicitHeight + 24
            radius: win.radius
            color: win.surfaceHigh
            border.color: win.line
            opacity: 0
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: 180 } }
            Label {
                id: toastLabel
                anchors.centerIn: parent
                width: parent.width - 40
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                color: win.textColor
            }
            Timer { id: toastTimer; interval: 3500; onTriggered: toastBox.opacity = 0 }
        }

        HelpOverlay {
            anchors.fill: parent
            visible: win.helpVisible
        }
    }

    Component { id: torrentPage; TorrentPage {} }
    Component { id: playerPage; PlayerPage {} }

    AddDialog { id: addDialog }
    DeleteDialog { id: deleteDialog }

    Dialog {
        id: quitDialog
        anchors.centerIn: parent
        modal: true
        title: qsTr("Quit Torrent Player?")
        width: Math.min(480, win.width - 64)
        padding: 24
        Material.roundedScale: theme.radius
        background: Rectangle { color: win.surface; radius: win.radius; border.color: win.line }
        Label {
            width: quitDialog.availableWidth
            wrapMode: Text.Wrap
            color: win.muted
            text: qsTr("%n downloads stop while Torrent Player is closed. They continue the next time you open it.", "", library.activeCount)
        }
        footer: DialogButtonBox {
            Button { text: qsTr("Cancel"); flat: true; DialogButtonBox.buttonRole: DialogButtonBox.RejectRole }
            Button { text: qsTr("Quit"); flat: true; DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole }
        }
        onAccepted: { win.quitConfirmed = true; Qt.quit() }
    }
}
