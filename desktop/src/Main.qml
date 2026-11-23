import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

ApplicationWindow {
    id: win
    width: 1180
    height: 760
    // Small enough for a tiled corner; PiP goes smaller still (a wide film is 480x201).
    minimumWidth: win.inPip ? 200 : 360
    minimumHeight: win.inPip ? 112 : 240
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
    // Picture-in-picture (PipWindow); null-safe, as the app tears down.
    readonly property bool inPip: pipWindow ? pipWindow.active : false
    // Below this the pages switch to their one-column, icon-button layouts.
    readonly property bool compact: width < 600
    // Side margin of page content: tight on a small window, roomier on a big one.
    readonly property int pageMargin: width < 600 ? 12 : 24
    // Dialog width: the preferred one, or the window less 16 px a side.
    function dialogWidth(preferred) { return Math.min(preferred, win.width - 32) }

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
    function openPeers(key) {
        stack.push(peersPage, { torrentKey: key })
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
    function openSettings() {
        if (stack.currentItem && stack.currentItem.isSettings)
            return
        stack.push(settingsPage)
    }
    function requestQuit() {
        if (library.activeCount > 0 && !quitConfirmed)
            quitDialog.open()
        else {
            quitConfirmed = true
            if (pipWindow) pipWindow.leave()
            Qt.quit()
        }
    }
    function hideToTray() {
        // A hidden PiP window would stay pinned and tiny; leave it first.
        if (win.inPip)
            pipWindow.leave()
        if (tray && tray.available)
            tray.hideToTray()
        else
            win.requestQuit()
    }

    onClosing: (close) => {
        if (quitConfirmed)
            return
        close.accepted = false
        win.hideToTray()
    }

    Connections {
        target: library
        function onPlayReady(media) {
            if (!settings.builtInPlayer) {
                library.openInMpv(media.url, media.startMs || 0, media.title || "")
                return
            }
            if (onPlayer)
                stack.pop()
            stack.push(playerPage, { media: media })
        }
        function onMessage(text) { win.toast(text) }
    }
    Connections {
        target: tray
        function onQuitRequested() { win.requestQuit() }
        function onFirstHideToTray() {
            win.toast(qsTr("Torrent Player keeps downloading in the background"))
        }
    }
    // Seeding pauses while the player page is open.
    onOnPlayerChanged: {
        deviceStatus.playing = win.onPlayer
        // PiP ends with the player; checked later, as the next episode swaps one player for another.
        Qt.callLater(win.endPipWithoutPlayer)
    }
    function endPipWithoutPlayer() {
        if (win.inPip && !win.onPlayer)
            if (pipWindow) pipWindow.leave()
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
            else if (kind === "directory") {
                library.setSaveDirectory(path)
                settings.downloadFolder = path
            }
            else if (onPlayer)
                stack.currentItem.loadSubtitleFile(path)
        }
        function onFailed(message) { win.toast(message) }
    }

    Shortcut { sequence: "?"; context: Qt.ApplicationShortcut; enabled: !addDialog.visible && !win.inPip; onActivated: win.helpVisible = !win.helpVisible }
    Shortcut { sequence: "Q"; context: Qt.ApplicationShortcut; enabled: !dialogOpen; onActivated: win.requestQuit() }
    Shortcut { sequence: "Ctrl+Q"; context: Qt.ApplicationShortcut; onActivated: win.requestQuit() }
    Shortcut { sequences: [",", "Ctrl+,"]; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: win.openSettings() }
    Shortcut { sequence: "A"; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: win.openAdd("") }
    Shortcut { sequences: [StandardKey.Paste]; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer && !(activeFocusItem instanceof TextInput); onActivated: win.openAdd(library.clipboardText()) }
    Shortcut { sequence: "O"; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: picker.open("torrent") }
    Shortcut { sequence: "Ctrl+O"; context: Qt.ApplicationShortcut; enabled: !dialogOpen && !onPlayer; onActivated: picker.open("torrent") }
    Shortcut {
        sequences: ["Esc", "Backspace", "Alt+Left"]
        context: Qt.ApplicationShortcut
        enabled: !dialogOpen && stack.depth > 1 && !(activeFocusItem instanceof TextInput)
        onActivated: {
            if (win.inPip)
                pipWindow.leave()
            else if (onPlayer && win.visibility === Window.FullScreen)
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
            visible: engine.error !== "" && !onPlayer && !win.inPip
            color: "#2a1215"
            RowLayout {
                id: engineBanner
                anchors { fill: parent; margins: 12; leftMargin: win.pageMargin; rightMargin: win.pageMargin }
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
            anchors { horizontalCenter: parent.horizontalCenter; bottom: parent.bottom; bottomMargin: win.height < 400 ? 12 : 32 }
            visible: opacity > 0 && !win.inPip
            width: Math.min(toastLabel.implicitWidth + 40, parent.width - 32)
            height: toastLabel.implicitHeight + 24
            radius: win.radius
            color: win.surfaceHigh
            border.color: win.line
            opacity: 0
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
    Component { id: peersPage; PeersPage {} }
    Component { id: settingsPage; SettingsPage { readonly property bool isSettings: true } }

    AddDialog { id: addDialog }
    DeleteDialog { id: deleteDialog }

    Dialog {
        id: quitDialog
        anchors.centerIn: parent
        modal: true
        title: qsTr("Quit Torrent Player?")
        width: win.dialogWidth(480)
        height: Math.min(implicitHeight, win.height - 32)
        padding: win.compact ? 16 : 24
        Material.roundedScale: theme.radius
        background: Rectangle { color: win.surface; radius: win.radius; border.color: win.line }
        // As the content item it takes the dialog's width, so its wrapped height
        // doesn't feed back into the dialog's size.
        contentItem: Flickable {
            implicitHeight: quitText.implicitHeight
            contentHeight: quitText.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            Label {
                id: quitText
                width: quitDialog.availableWidth
                wrapMode: Text.Wrap
                color: win.muted
                text: qsTr("%n downloads stop while Torrent Player is closed. They continue the next time you open it.", "", library.activeCount)
            }
        }
        footer: DialogButtonBox {
            Button { text: qsTr("Cancel"); flat: true; DialogButtonBox.buttonRole: DialogButtonBox.RejectRole }
            Button { text: qsTr("Quit"); flat: true; DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole }
        }
        onAccepted: { win.quitConfirmed = true; if (pipWindow) pipWindow.leave(); Qt.quit() }
    }
}
