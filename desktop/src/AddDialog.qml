import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Dialog {
    id: dialog
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: win.dialogWidth(760)
    height: adder.state === "choose" ? Math.min(680, win.height - 32) : Math.min(implicitHeight, win.height - 32)
    modal: true
    padding: narrow ? 16 : 24
    // Narrow: button rows wrap and the rarer buttons go under the main ones.
    readonly property bool narrow: width < 560
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
        padding: dialog.narrow ? 16 : 24
        bottomPadding: 0
        elide: Text.ElideRight
        text: adder.state === "choose" ? qsTr("Choose files") : qsTr("Add download")
        font.pixelSize: 20
        font.weight: Font.DemiBold
        color: win.textColor
    }

    // Scrolls when the window is too short for it.
    contentItem: Flickable {
        id: addFlick
        implicitHeight: content.implicitHeight
        contentWidth: width
        contentHeight: content.height
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar {}
        ColumnLayout {
            id: content
            width: addFlick.width
            height: Math.max(addFlick.height, implicitHeight)
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
                GridLayout {
                    Layout.fillWidth: true
                    columns: dialog.narrow ? 1 : 2
                    rowSpacing: 0
                    Flow {
                        Layout.fillWidth: true
                        spacing: 4
                        Button { text: qsTr("Paste link"); flat: true; onClicked: { input.text = library.clipboardText(); if (input.text !== "") adder.start(input.text) } }
                        Button { text: qsTr("Open torrent file"); flat: true; onClicked: picker.open("torrent") }
                    }
                    // Right-aligned, wrapping from the right: Continue stays in the corner.
                    Flow {
                        Layout.fillWidth: true
                        layoutDirection: Qt.RightToLeft
                        spacing: 4
                        Button {
                            text: qsTr("Continue")
                            highlighted: true
                            enabled: input.text.trim() !== ""
                            onClicked: adder.start(input.text)
                        }
                        Button { text: qsTr("Cancel"); flat: true; onClicked: dialog.close() }
                        Button {
                            visible: adder.existingKey !== ""
                            text: qsTr("Open existing torrent")
                            flat: true
                            onClicked: { const key = adder.existingKey; dialog.close(); win.openTorrent(key) }
                        }
                    }
                }
            }

            // ------------------------------------------------ waiting
            ColumnLayout {
                Layout.fillWidth: true
                visible: adder.state === "starting" || adder.state === "loading" || adder.state === "fetching" || adder.state === "saving"
                spacing: 12
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 16
                    BusyIndicator { running: parent.visible }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        Label {
                            Layout.fillWidth: true
                            wrapMode: Text.Wrap
                            color: win.textColor
                            font.pixelSize: 15
                            text: adder.state === "starting" ? qsTr("Starting download engine…")
                                  : adder.state === "loading" ? qsTr("Downloading the torrent file…")
                                  : adder.state === "saving" ? qsTr("Adding to your library…")
                                  : qsTr("Finding files")
                        }
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
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
                GridLayout {
                    Layout.fillWidth: true
                    columns: dialog.narrow ? 1 : 2
                    rowSpacing: 0
                    Flow {
                        Layout.fillWidth: true
                        spacing: 4
                        Button { text: qsTr("Select all"); flat: true; onClicked: adder.selectAll() }
                        Button { text: qsTr("Select none"); flat: true; onClicked: adder.selectNone() }
                        Button { text: qsTr("Videos only"); flat: true; onClicked: adder.selectVideos() }
                    }
                    Label {
                        Layout.fillWidth: true
                        horizontalAlignment: dialog.narrow ? Text.AlignLeft : Text.AlignRight
                        wrapMode: Text.Wrap
                        text: qsTr("Videos and matching subtitles start selected.")
                        color: win.muted
                        font.pixelSize: 12
                    }
                }
                Rectangle {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    Layout.minimumHeight: Math.min(160, Math.max(96, win.height * 0.4))
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
                    wrapMode: Text.Wrap
                    text: qsTr("%1 of %2 files selected · %3").arg(adder.selectedCount).arg(dialog.shownFiles.length).arg(fmt.bytes(adder.selectedBytes))
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.selectedBytes > adder.freeBytes
                    color: win.danger
                    wrapMode: Text.Wrap
                    text: adder.selectedHasVideo
                          ? qsTr("Not enough storage to download %1 (%2 free). Play now streams without saving.").arg(fmt.bytes(adder.selectedBytes)).arg(fmt.bytes(adder.freeBytes))
                          : qsTr("Not enough storage to download %1 (%2 free).").arg(fmt.bytes(adder.selectedBytes)).arg(fmt.bytes(adder.freeBytes))
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.selectedHasVideo
                    color: win.muted
                    wrapMode: Text.Wrap
                    font.pixelSize: 12
                    text: qsTr("Download saves the files to your Downloads folder. Play now (Direct Play) streams the video from memory and saves nothing.")
                }
                Label {
                    Layout.fillWidth: true
                    visible: adder.selectedCount === 0
                    wrapMode: Text.Wrap
                    color: win.danger
                    text: qsTr("Select at least one file to continue.")
                }
                GridLayout {
                    Layout.fillWidth: true
                    columns: dialog.narrow ? 1 : 2
                    rowSpacing: 0
                    Label {
                        Layout.fillWidth: true
                        elide: Text.ElideMiddle
                        color: win.muted
                        font.pixelSize: 12
                        text: qsTr("Saved to %1").arg(library.saveDirectory)
                    }
                    // Right-aligned, wrapping from the right: the main choices stay in the corner.
                    Flow {
                        Layout.fillWidth: true
                        Layout.minimumWidth: dialog.narrow ? 0 : 320
                        layoutDirection: Qt.RightToLeft
                        spacing: 4
                        Button {
                            text: qsTr("Play now")
                            flat: !settings.directPlayDefault
                            highlighted: settings.directPlayDefault
                            visible: adder.selectedHasVideo
                            enabled: adder.selectedCount > 0
                            ToolTip.text: qsTr("Direct Play: stream only, nothing saved to Downloads")
                            ToolTip.visible: hovered
                            ToolTip.delay: 600
                            onClicked: adder.confirm(true)
                        }
                        // The default choice (Settings: When adding a torrent) is the highlighted one.
                        Button {
                            readonly property bool preferred: !(settings.directPlayDefault && adder.selectedHasVideo)
                            text: qsTr("Download")
                            flat: !preferred
                            highlighted: preferred
                            enabled: adder.selectedCount > 0 && adder.selectedBytes <= adder.freeBytes
                            onClicked: adder.confirm(false)
                        }
                        Button { text: qsTr("Cancel"); flat: true; onClicked: dialog.close() }
                    }
                }
            }
        }
    }
}
