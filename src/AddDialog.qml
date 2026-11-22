import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Dialog {
    id: dialog
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(760, win.width - 64)
    height: adder.state === "choose" ? Math.min(680, win.height - 64) : implicitHeight
    modal: true
    padding: 24
    Material.roundedScale: theme.radius
    background: Rectangle {
        color: win.surface
        radius: win.radius
        border.color: win.line
    }
    closePolicy: Popup.CloseOnEscape

    function openWith(text) {
        input.text = text
        open()
        // A link handed in (pasted, dropped, opened from the browser) starts right away.
        if (text !== "")
            adder.start(text)
        else
            input.forceActiveFocus()
    }

    onClosed: {
        if (adder.state !== "idle")
            adder.cancel()
        input.text = ""
    }

    readonly property var shownFiles: adder.files

    header: Label {
        padding: 24
        bottomPadding: 0
        text: adder.state === "choose" ? qsTr("Choose files") : qsTr("Add download")
        font.pixelSize: 20
        font.weight: Font.DemiBold
        color: win.textColor
    }

    contentItem: Item {
        implicitHeight: content.implicitHeight
        ColumnLayout {
            id: content
            anchors.fill: parent
            spacing: 14

            // ------------------------------------------------ the link
            ColumnLayout {
                Layout.fillWidth: true
                visible: adder.state === "idle" || adder.state === "error"
                spacing: 12
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    color: win.muted
                    text: qsTr("Paste a magnet link or a link to a .torrent file, or open a .torrent file from your computer.")
                }
                TextField {
                    id: input
                    Layout.fillWidth: true
                    placeholderText: qsTr("Magnet link or torrent URL")
                    onAccepted: if (text.trim() !== "") adder.start(text)
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.state === "error"
                    text: adder.error
                    color: win.danger
                    wrapMode: Text.Wrap
                }
                RowLayout {
                    Layout.fillWidth: true
                    Button { text: qsTr("Paste link"); flat: true; onClicked: { input.text = library.clipboardText(); if (input.text !== "") adder.start(input.text) } }
                    Button { text: qsTr("Open torrent file"); flat: true; onClicked: picker.open("torrent") }
                    Item { Layout.fillWidth: true }
                    Button {
                        visible: adder.existingKey !== ""
                        text: qsTr("Open existing torrent")
                        flat: true
                        onClicked: { const key = adder.existingKey; dialog.close(); win.openTorrent(key) }
                    }
                    Button { text: qsTr("Cancel"); flat: true; onClicked: dialog.close() }
                    Button {
                        text: qsTr("Continue")
                        highlighted: true
                        enabled: input.text.trim() !== ""
                        onClicked: adder.start(input.text)
                    }
                }
            }

            // ------------------------------------------------ waiting
            ColumnLayout {
                Layout.fillWidth: true
                visible: adder.state === "starting" || adder.state === "loading" || adder.state === "fetching" || adder.state === "saving"
                spacing: 12
                RowLayout {
                    spacing: 16
                    BusyIndicator { running: parent.visible }
                    ColumnLayout {
                        spacing: 4
                        Label {
                            color: win.textColor
                            font.pixelSize: 15
                            text: adder.state === "starting" ? qsTr("Starting download engine…")
                                  : adder.state === "loading" ? qsTr("Downloading the torrent file…")
                                  : adder.state === "saving" ? qsTr("Adding to your library…")
                                  : qsTr("Finding files")
                        }
                        Label {
                            visible: adder.state === "fetching"
                            color: win.muted
                            text: adder.name !== "" ? adder.name : qsTr("Asking peers for the file list · %n peers", "", adder.peers)
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.state === "fetching" && adder.waitedSeconds > 20 && adder.peers === 0
                    wrapMode: Text.Wrap
                    color: win.muted
                    text: qsTr("No peers yet. Torrents with few seeders can take a while to find; keep this open or try again later.")
                }
                RowLayout {
                    Item { Layout.fillWidth: true }
                    Button { text: qsTr("Cancel"); flat: true; onClicked: dialog.close() }
                }
            }

            // ------------------------------------------------ choosing files
            ColumnLayout {
                Layout.fillWidth: true
                Layout.fillHeight: true
                visible: adder.state === "choose"
                spacing: 10

                Label {
                    Layout.fillWidth: true
                    text: adder.name
                    color: win.textColor
                    font.pixelSize: 15
                    font.weight: Font.Medium
                    elide: Text.ElideMiddle
                }
                RowLayout {
                    spacing: 4
                    Button { text: qsTr("Select all"); flat: true; onClicked: adder.selectAll() }
                    Button { text: qsTr("Select none"); flat: true; onClicked: adder.selectNone() }
                    Button { text: qsTr("Videos only"); flat: true; onClicked: adder.selectVideos() }
                    Item { Layout.fillWidth: true }
                    Label { text: qsTr("Videos and matching subtitles start selected."); color: win.muted; font.pixelSize: 12 }
                }
                Rectangle {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.minimumHeight: 160
                    radius: win.radius
                    color: win.surface
                    border.color: win.line
                    clip: true
                    ListView {
                        id: fileList
                        anchors.fill: parent
                        anchors.margins: 1
                        clip: true
                        // A count model keeps the scroll position when a tick changes the list.
                        model: dialog.shownFiles.length
                        ScrollBar.vertical: ScrollBar {}
                        delegate: ItemDelegate {
                            id: row
                            required property int index
                            readonly property var file: dialog.shownFiles[index] || ({})
                            width: ListView.view.width
                            onClicked: adder.toggle(row.file.index)
                            contentItem: RowLayout {
                                spacing: 12
                                CheckBox {
                                    checked: row.file.selected === true
                                    onClicked: adder.toggle(row.file.index)
                                }
                                Label {
                                    Layout.fillWidth: true
                                    text: row.file.path || ""
                                    elide: Text.ElideMiddle
                                    color: row.file.selected ? win.textColor : win.muted
                                }
                                Label {
                                    text: fmt.bytes(row.file.length || 0)
                                    color: win.muted
                                    font.pixelSize: 12
                                    Layout.rightMargin: 8
                                }
                            }
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    color: win.muted
                    text: qsTr("%1 of %2 files selected · %3").arg(adder.selectedCount).arg(dialog.shownFiles.length).arg(fmt.bytes(adder.selectedBytes))
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.selectedBytes > adder.freeBytes
                    color: win.danger
                    wrapMode: Text.Wrap
                    text: qsTr("Not enough storage to download %1 (%2 free).").arg(fmt.bytes(adder.selectedBytes)).arg(fmt.bytes(adder.freeBytes))
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.selectedCount === 0
                    color: win.danger
                    text: qsTr("Select at least one file to continue.")
                }
                RowLayout {
                    Layout.fillWidth: true
                    Label {
                        Layout.fillWidth: true
                        elide: Text.ElideMiddle
                        color: win.muted
                        font.pixelSize: 12
                        text: qsTr("Saved to %1").arg(library.saveDirectory)
                    }
                    Button { text: qsTr("Cancel"); flat: true; onClicked: dialog.close() }
                    Button {
                        text: qsTr("Download")
                        flat: adder.selectedHasVideo
                        highlighted: !adder.selectedHasVideo
                        enabled: adder.selectedCount > 0
                        onClicked: adder.confirm(false)
                    }
                    Button {
                        text: qsTr("Play now")
                        highlighted: true
                        visible: adder.selectedHasVideo
                        enabled: adder.selectedCount > 0
                        onClicked: adder.confirm(true)
                    }
                }
            }
        }
    }
}
