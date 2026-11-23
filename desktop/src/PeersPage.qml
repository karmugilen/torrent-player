import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import Webtor 1.0

Page {
    id: page
    property string torrentKey
    readonly property var info: library.revision >= 0 ? library.details(torrentKey) : ({})
    // Prefer details.engineId when present; otherwise Connections resolves via
    // library.entry(key).id (engine ids are UUIDs), then the info-hash key.
    readonly property string engineId: (info && info.engineId) ? info.engineId : ""
    readonly property string pageTitle: info.name ? qsTr("Peers · %1").arg(info.name) : qsTr("Peers")
    readonly property color accentColor: theme.accent
    // Narrow windows drop the less useful columns; a huge one keeps content readable.
    readonly property int bodyWidth: Math.min(width - 32, 1400)
    readonly property bool showFlags: bodyWidth >= 640
    readonly property bool showAddress: bodyWidth >= 520
    readonly property bool showHave: bodyWidth >= 400
    background: Rectangle { color: "#0e0e10" }

    TorrentConnections {
        id: swarm
        engine: engine
        library: library
        torrentKey: page.torrentKey
        engineId: page.engineId
        active: page.StackView.status === StackView.Active && page.visible
    }

    function sortMarker(role) {
        if (swarm.sortRole !== role)
            return ""
        return swarm.sortAscending ? " ↑" : " ↓"
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.margins: win.compact ? 8 : 16
            Layout.bottomMargin: 8
            spacing: 8
            ToolButton {
                text: "←"
                font.pixelSize: 20
                ToolTip.text: qsTr("Back")
                ToolTip.visible: hovered
                onClicked: page.StackView.view.pop()
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2
                Label {
                    Layout.fillWidth: true
                    text: page.pageTitle
                    elide: Text.ElideMiddle
                    font.pixelSize: win.compact ? 17 : 20
                    font.weight: Font.DemiBold
                    color: win.textColor
                }
                Label {
                    Layout.fillWidth: true
                    text: swarm.status
                    color: win.muted
                    font.pixelSize: 13
                    elide: Text.ElideRight
                }
            }
            Rectangle {
                radius: win.smallRadius
                color: swarm.peerCount > 0 ? Qt.rgba(page.accentColor.r, page.accentColor.g, page.accentColor.b, 0.12) : "#1a1a1e"
                implicitWidth: liveLabel.implicitWidth + 16
                implicitHeight: liveLabel.implicitHeight + 10
                Label {
                    id: liveLabel
                    anchors.centerIn: parent
                    text: swarm.peerCount > 0 ? qsTr("LIVE") : qsTr("IDLE")
                    color: swarm.peerCount > 0 ? theme.accent : win.muted
                    font.family: "monospace"
                    font.pixelSize: 11
                    font.letterSpacing: 1
                }
            }
        }

        Flickable {
            id: flick
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: width
            contentHeight: body.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            ColumnLayout {
                id: body
                x: Math.round((flick.width - width) / 2)
                width: page.bodyWidth + 32
                spacing: 12

                WorldMap {
                    id: map
                    // 2:1 like the projection, and no taller than most of the window.
                    readonly property int mapWidth: Math.round(Math.min(page.bodyWidth, Math.max(280, flick.height * 0.7) * 2))
                    Layout.alignment: Qt.AlignHCenter
                    Layout.preferredWidth: mapWidth
                    Layout.preferredHeight: Math.round(mapWidth / 2)
                    countries: swarm.countries
                    accent: theme.accent
                    land: "#2a2a2f"
                    background: "#111114"
                    grid: "#3a3a42"
                }

                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    text: map.summary
                    color: win.muted
                    font.pixelSize: 13
                    wrapMode: Text.Wrap
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    text: qsTr("Country estimates · tap a marker")
                    color: win.muted
                    wrapMode: Text.Wrap
                    font.pixelSize: 12
                }
                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    text: qsTr("IP Geolocation by DB-IP · offline data, Oct 2026")
                    color: theme.accent
                    wrapMode: Text.Wrap
                    font.pixelSize: 12
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    Layout.bottomMargin: 16
                    radius: win.radius
                    color: win.surface
                    border.color: win.line
                    implicitHeight: headerRow.implicitHeight + peerList.contentHeight + 16

                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 8
                        spacing: 0

                        RowLayout {
                            id: headerRow
                            Layout.fillWidth: true
                            spacing: 0
                            Repeater {
                                model: [
                                    { role: "country", label: qsTr("CC"), width: 44 },
                                    { role: "client", label: qsTr("Client"), width: 0, fill: true },
                                    { role: "flags", label: qsTr("Flags"), width: 110, shown: page.showFlags },
                                    { role: "downloadRate", label: qsTr("Down"), width: 88 },
                                    { role: "progress", label: qsTr("Have"), width: 64, shown: page.showHave },
                                    { role: "displayAddress", label: qsTr("Address"), width: page.bodyWidth < 700 ? 130 : 180, shown: page.showAddress }
                                ]
                                delegate: AbstractButton {
                                    visible: modelData.shown !== false
                                    Layout.preferredWidth: modelData.width || 0
                                    Layout.fillWidth: !!modelData.fill
                                    Layout.preferredHeight: 32
                                    onClicked: swarm.toggleSort(modelData.role)
                                    contentItem: Label {
                                        text: modelData.label + page.sortMarker(modelData.role)
                                        color: win.muted
                                        font.pixelSize: 12
                                        font.weight: Font.DemiBold
                                        elide: Text.ElideRight
                                        verticalAlignment: Text.AlignVCenter
                                    }
                                }
                            }
                        }

                        Rectangle { Layout.fillWidth: true; height: 1; color: win.line }

                        ListView {
                            id: peerList
                            Layout.fillWidth: true
                            implicitHeight: contentHeight
                            interactive: false
                            clip: true
                            model: swarm.peers
                            delegate: Rectangle {
                                width: peerList.width
                                height: 40
                                color: index % 2 ? "#141417" : "transparent"
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 4
                                    anchors.rightMargin: 4
                                    spacing: 0
                                    Label {
                                        Layout.preferredWidth: 44
                                        text: model.country || "—"
                                        color: win.muted
                                        font.pixelSize: 12
                                        font.family: "monospace"
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        Layout.fillWidth: true
                                        text: model.client
                                        color: win.textColor
                                        font.pixelSize: 13
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        visible: page.showFlags
                                        Layout.preferredWidth: 110
                                        text: model.flags
                                        color: win.muted
                                        font.pixelSize: 12
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        Layout.preferredWidth: 88
                                        text: model.downloadRate >= 0 ? fmt.speed(model.downloadRate) : "—"
                                        color: win.muted
                                        font.pixelSize: 12
                                        font.family: "monospace"
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        visible: page.showHave
                                        Layout.preferredWidth: 64
                                        text: model.progress >= 0 ? fmt.percent(model.progress) : "—"
                                        color: win.muted
                                        font.pixelSize: 12
                                        font.family: "monospace"
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        visible: page.showAddress
                                        Layout.preferredWidth: page.bodyWidth < 700 ? 130 : 180
                                        text: model.displayAddress
                                        color: win.muted
                                        font.pixelSize: 12
                                        font.family: "monospace"
                                        elide: Text.ElideMiddle
                                    }
                                }
                            }
                        }

                        Label {
                            Layout.fillWidth: true
                            Layout.margins: 12
                            visible: swarm.peerCount === 0
                            horizontalAlignment: Text.AlignHCenter
                            text: swarm.status
                            color: win.muted
                        }
                    }
                }
            }
        }
    }
}
