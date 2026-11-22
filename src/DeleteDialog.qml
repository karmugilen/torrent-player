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
    width: Math.min(480, win.width - 64)
    modal: true
    title: qsTr("Remove this download?")
    padding: 24
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

    ColumnLayout {
        width: parent.width
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
