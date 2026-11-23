import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts

Dialog {
    id: dialog
    property string key
    property string name
    parent: Overlay.overlay
    anchors.centerIn: parent
    width: win.dialogWidth(480)
    height: Math.min(implicitHeight, win.height - 32)
    modal: true
    title: qsTr("Remove this download?")
    padding: win.compact ? 16 : 24
    Material.roundedScale: theme.radius
    background: Rectangle {
        color: win.surface
        radius: win.radius
        border.color: win.line
    }

    function ask(torrentKey) {
        key = torrentKey
        name = library.details(torrentKey).name || ""
        open()
    }

    // Scrolls when the window is too short for it.
    contentItem: Flickable {
        id: deleteFlick
        implicitHeight: deleteText.implicitHeight
        contentWidth: width
        contentHeight: deleteText.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        ScrollBar.vertical: ScrollBar {}
    ColumnLayout {
        id: deleteText
        width: deleteFlick.width
        spacing: 12
        Label {
            Layout.fillWidth: true
            text: "“" + (dialog.name.length > 80 ? dialog.name.slice(0, 80) + "…" : dialog.name) + "”"
            color: win.textColor
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            color: win.muted
            text: qsTr("Permanently erase the downloaded files from your computer, or remove from the library while keeping files?")
        }
    }
    }

    footer: DialogButtonBox {
        Button { text: qsTr("Cancel"); flat: true; DialogButtonBox.buttonRole: DialogButtonBox.RejectRole }
        Button {
            text: qsTr("Keep files")
            flat: true
            onClicked: { library.remove(dialog.key, false); dialog.close(); dialog.done() }
        }
        Button {
            text: qsTr("Delete files")
            flat: true
            Material.foreground: win.danger
            onClicked: { library.remove(dialog.key, true); dialog.close(); dialog.done() }
        }
    }

    // After removing from the torrent page, go back to the library.
    function done() { win.goHome() }
}
