import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import QtMultimedia

Page {
    id: page
    readonly property bool isPlayer: true
    property var media: ({})
    readonly property string torrentKey: media.key || ""
    readonly property string pageTitle: media.title || ""
    readonly property bool active: StackView.status === StackView.Active && !win.dialogOpen
    property bool resumed: false
    property bool controlsVisible: true
    property bool thumbnailTaken: false
    property int subtitleChoice: -1      // torrent subtitle index, -1 none
    property string subtitleLabel: ""
    readonly property var torrent: library.revision >= 0 ? library.details(torrentKey) : ({})
    background: Rectangle { color: "black" }

    function showControls() {
        controlsVisible = true
        hideTimer.restart()
    }
    function seekBy(ms) {
        player.position = Math.max(0, Math.min(player.duration, player.position + ms))
        showControls()
        osd.show((ms > 0 ? "+" : "−") + Math.abs(ms / 1000) + " s")
    }
    function togglePlay() {
        if (player.playbackState === MediaPlayer.PlayingState) player.pause()
        else player.play()
        showControls()
    }
    function toggleFullScreen() {
        if (win.visibility === Window.FullScreen) win.showNormal()
        else win.showFullScreen()
    }
    function loadSubtitleFile(path) {
        player.activeSubtitleTrack = -1
        subtitleChoice = -2
        subtitleLabel = path.split("/").pop()
        subtitles.load(path, subtitleLabel)
    }
    function chooseTorrentSubtitle(sub) {
        player.activeSubtitleTrack = -1
        subtitleChoice = sub.index
        subtitleLabel = sub.label
        if (sub.ready) {
            subtitles.load(sub.path, sub.label)
        } else {
            // Not downloaded yet: fetch it (it's small) and load it when it lands.
            library.setFileSelected(torrentKey, sub.index, true)
            subtitles.clear()
            pendingSubtitle.restart()
            osd.show(qsTr("Downloading %1…").arg(sub.label))
        }
    }
    function subtitlesOff() {
        subtitleChoice = -1
        subtitleLabel = ""
        subtitles.clear()
        player.activeSubtitleTrack = -1
    }
    function cycleSubtitles() {
        // Off → each torrent subtitle → each embedded track → off.
        const list = media.subtitles || []
        const embedded = player.subtitleTracks.length
        let order = [{ kind: "off" }]
        for (const s of list) order.push({ kind: "file", sub: s })
        for (let i = 0; i < embedded; ++i) order.push({ kind: "track", track: i })
        let current = 0
        for (let i = 0; i < order.length; ++i) {
            const o = order[i]
            if ((o.kind === "file" && o.sub.index === subtitleChoice) ||
                (o.kind === "track" && o.track === player.activeSubtitleTrack && subtitleChoice === -1))
                current = i
        }
        const next = order[(current + 1) % order.length]
        if (next.kind === "off") { subtitlesOff(); osd.show(qsTr("Subtitles off")) }
        else if (next.kind === "file") { chooseTorrentSubtitle(next.sub); osd.show(next.sub.label) }
        else { subtitlesOff(); player.activeSubtitleTrack = next.track; osd.show(trackName(player.subtitleTracks[next.track], next.track)) }
    }
    function trackName(meta, i) {
        const lang = meta ? meta.stringValue(MediaMetaData.Language) : ""
        const title = meta ? meta.stringValue(MediaMetaData.Title) : ""
        return [title, lang].filter(x => x && x !== "").join(" · ") || qsTr("Track %1").arg(i + 1)
    }
    function leave() {
        // Near the end counts as watched, so the next play starts over.
        const position = player.duration > 0 && player.position > player.duration * 0.95 ? 0 : player.position
        library.savePosition(torrentKey, media.fileIndex, position)
        player.stop()
        subtitles.clear()
        if (win.visibility === Window.FullScreen)
            win.showNormal()
    }

    StackView.onRemoved: leave()
    Component.onDestruction: if (player.playbackState !== MediaPlayer.StoppedState) leave()

    MediaPlayer {
        id: player
        source: page.media.url || ""
        audioOutput: AudioOutput { id: audio; volume: 1.0 }
        videoOutput: video
        onMediaStatusChanged: {
            if (mediaStatus === MediaPlayer.LoadedMedia || mediaStatus === MediaPlayer.BufferedMedia) {
                if (!page.resumed) {
                    page.resumed = true
                    const start = page.media.startMs || 0
                    if (start > 5000 && (duration <= 0 || start < duration - 10000)) {
                        position = start
                        osd.show(qsTr("Resumed at %1").arg(fmt.clock(start)))
                    }
                    play()
                }
            }
        }
        onErrorOccurred: (error, errorString) => errorBox.message = errorString
        Component.onCompleted: play()
    }

    VideoOutput {
        id: video
        anchors.fill: parent
        fillMode: VideoOutput.PreserveAspectFit
    }

    // A frame for the home card, once real picture is on screen.
    Timer {
        interval: 8000
        running: player.playbackState === MediaPlayer.PlayingState && !page.thumbnailTaken
        onTriggered: { page.thumbnailTaken = true; library.captureThumbnail(page.torrentKey, video.videoSink) }
    }
    Timer {
        id: pendingSubtitle
        interval: 1000
        repeat: true
        onTriggered: {
            const path = library.finishedFilePath(page.torrentKey, page.subtitleChoice)
            if (path !== "") {
                stop()
                subtitles.load(path, page.subtitleLabel)
            }
        }
    }

    // External subtitles.
    Label {
        anchors { horizontalCenter: parent.horizontalCenter; bottom: parent.bottom; bottomMargin: controlsVisible ? 120 : 48 }
        width: parent.width * 0.8
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        text: subtitles.loaded ? subtitles.textAt(player.position) : ""
        color: "white"
        font.pixelSize: Math.max(20, page.height / 24)
        style: Text.Outline
        styleColor: "black"
        visible: text !== ""
        Behavior on anchors.bottomMargin { NumberAnimation { duration: 150 } }
    }

    // Buffering and errors.
    BusyIndicator {
        anchors.centerIn: parent
        running: errorBox.message === "" && (player.mediaStatus === MediaPlayer.LoadingMedia
                 || player.mediaStatus === MediaPlayer.BufferingMedia
                 || player.mediaStatus === MediaPlayer.StalledMedia)
        visible: running
    }
    Label {
        anchors { horizontalCenter: parent.horizontalCenter; top: parent.verticalCenter; topMargin: 48 }
        visible: errorBox.message === "" && (player.mediaStatus === MediaPlayer.LoadingMedia
                 || player.mediaStatus === MediaPlayer.StalledMedia || player.mediaStatus === MediaPlayer.BufferingMedia)
        text: qsTr("Buffering · %1 · %n peers", "", page.torrent.peers || 0).arg(fmt.speed(page.torrent.downloadSpeed || 0))
        color: "#cccccc"
    }
    Rectangle {
        id: errorBox
        property string message: ""
        anchors.centerIn: parent
        visible: message !== ""
        width: Math.min(520, parent.width - 64)
        height: errorColumn.implicitHeight + 40
        radius: win.radius
        color: win.surface
        border.color: win.line
        ColumnLayout {
            id: errorColumn
            anchors { fill: parent; margins: 20 }
            spacing: 12
            Label { text: qsTr("The built-in player can't play this file"); color: win.textColor; font.pixelSize: 16; font.weight: Font.Medium }
            Label { Layout.fillWidth: true; text: errorBox.message; color: win.muted; wrapMode: Text.Wrap }
            RowLayout {
                Button { text: qsTr("Open in mpv"); highlighted: true; onClicked: library.openInMpv(page.media.url, player.position, page.pageTitle) }
                Button { text: qsTr("Back"); flat: true; onClicked: page.StackView.view.pop() }
            }
        }
    }

    // Short on-screen messages (seek steps, volume, subtitle changes).
    Rectangle {
        id: osd
        function show(text) { osdLabel.text = text; opacity = 1; osdTimer.restart() }
        anchors { top: parent.top; horizontalCenter: parent.horizontalCenter; topMargin: 72 }
        width: osdLabel.implicitWidth + 32
        height: osdLabel.implicitHeight + 16
        radius: win.radius
        color: Qt.rgba(0, 0, 0, 0.7)
        opacity: 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 150 } }
        Label { id: osdLabel; anchors.centerIn: parent; color: "white"; font.pixelSize: 16 }
        Timer { id: osdTimer; interval: 1200; onTriggered: osd.opacity = 0 }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: controlsVisible ? Qt.ArrowCursor : Qt.BlankCursor
        acceptedButtons: Qt.LeftButton
        onPositionChanged: page.showControls()
        onClicked: page.togglePlay()
        onDoubleClicked: { page.togglePlay(); page.toggleFullScreen() }
        onWheel: (wheel) => {
            audio.volume = Math.max(0, Math.min(1, audio.volume + (wheel.angleDelta.y > 0 ? 0.05 : -0.05)))
            osd.show(qsTr("Volume %1").arg(fmt.percent(audio.volume)))
        }
    }
    Timer {
        id: hideTimer
        interval: 2500
        onTriggered: if (player.playbackState === MediaPlayer.PlayingState && !subtitleMenu.visible && !audioMenu.visible && !seek.pressed) page.controlsVisible = false
    }

    // Top bar.
    Rectangle {
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 64
        opacity: controlsVisible ? 1 : 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 200 } }
        gradient: Gradient {
            GradientStop { position: 0; color: Qt.rgba(0, 0, 0, 0.75) }
            GradientStop { position: 1; color: "transparent" }
        }
        RowLayout {
            anchors { fill: parent; leftMargin: 12; rightMargin: 16 }
            ToolButton { text: "←"; font.pixelSize: 20; onClicked: page.StackView.view.pop() }
            Label { Layout.fillWidth: true; text: page.pageTitle; color: "white"; elide: Text.ElideMiddle; font.pixelSize: 16 }
            Label {
                visible: (page.torrent.progress || 0) < 1
                text: "↓ " + fmt.speed(page.torrent.downloadSpeed || 0) + " · " + fmt.percent(page.torrent.progress || 0)
                color: "#cccccc"
                font.pixelSize: 13
            }
        }
    }

    // Bottom controls.
    Rectangle {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 104
        opacity: controlsVisible ? 1 : 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 200 } }
        gradient: Gradient {
            GradientStop { position: 0; color: "transparent" }
            GradientStop { position: 1; color: Qt.rgba(0, 0, 0, 0.85) }
        }
        ColumnLayout {
            anchors { fill: parent; leftMargin: 20; rightMargin: 20; bottomMargin: 8; topMargin: 20 }
            spacing: 0
            RowLayout {
                Layout.fillWidth: true
                spacing: 12
                Label { text: fmt.clock(seek.pressed ? seek.value : player.position); color: "white"; font.pixelSize: 13 }
                Slider {
                    id: seek
                    Layout.fillWidth: true
                    from: 0
                    to: Math.max(1, player.duration)
                    enabled: player.seekable
                    value: pressed ? value : player.position
                    onPressedChanged: if (!pressed) player.position = value
                    onMoved: page.showControls()
                }
                Label { text: fmt.clock(player.duration); color: "white"; font.pixelSize: 13 }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 4
                ToolButton { text: "⟲ 10"; font.pixelSize: 13; onClicked: page.seekBy(-10000) }
                ToolButton {
                    text: player.playbackState === MediaPlayer.PlayingState ? "❚❚" : "▶"
                    font.pixelSize: 18
                    onClicked: page.togglePlay()
                }
                ToolButton { text: "10 ⟳"; font.pixelSize: 13; onClicked: page.seekBy(10000) }
                ToolButton {
                    text: audio.muted || audio.volume === 0 ? qsTr("Muted") : qsTr("Volume")
                    font.pixelSize: 13
                    onClicked: audio.muted = !audio.muted
                }
                Slider {
                    Layout.preferredWidth: 110
                    from: 0; to: 1
                    value: audio.volume
                    onMoved: { audio.volume = value; audio.muted = false }
                }
                Item { Layout.fillWidth: true }
                ToolButton {
                    text: page.subtitleLabel !== "" ? qsTr("Subtitles: %1").arg(page.subtitleLabel)
                          : player.activeSubtitleTrack >= 0 ? qsTr("Subtitles: %1").arg(page.trackName(player.subtitleTracks[player.activeSubtitleTrack], player.activeSubtitleTrack))
                          : qsTr("Subtitles")
                    font.pixelSize: 13
                    onClicked: subtitleMenu.popup()
                    Layout.maximumWidth: 280
                }
                ToolButton {
                    visible: player.audioTracks.length > 1
                    text: qsTr("Audio")
                    font.pixelSize: 13
                    onClicked: audioMenu.popup()
                }
                ToolButton {
                    text: qsTr("Open in mpv")
                    font.pixelSize: 13
                    onClicked: { if (library.openInMpv(page.media.url, player.position, page.pageTitle)) player.pause() }
                }
                ToolButton {
                    text: win.visibility === Window.FullScreen ? qsTr("Exit full screen") : qsTr("Full screen")
                    font.pixelSize: 13
                    onClicked: page.toggleFullScreen()
                }
            }
        }
    }

    Menu {
        id: subtitleMenu
        Material.roundedScale: theme.radius
        MenuItem { text: qsTr("Off"); checkable: true; checked: page.subtitleChoice === -1 && player.activeSubtitleTrack < 0; onTriggered: page.subtitlesOff() }
        Repeater {
            model: page.media.subtitles || []
            delegate: MenuItem {
                text: modelData.ready ? modelData.label : qsTr("%1 (download)").arg(modelData.label)
                checkable: true
                checked: page.subtitleChoice === modelData.index
                onTriggered: page.chooseTorrentSubtitle(modelData)
            }
        }
        Repeater {
            model: player.subtitleTracks.length
            delegate: MenuItem {
                text: qsTr("Embedded: %1").arg(page.trackName(player.subtitleTracks[index], index))
                checkable: true
                checked: page.subtitleChoice === -1 && player.activeSubtitleTrack === index
                onTriggered: { page.subtitlesOff(); player.activeSubtitleTrack = index }
            }
        }
        MenuSeparator {}
        MenuItem { text: qsTr("Open subtitle file…"); onTriggered: picker.open("subtitle") }
    }
    Menu {
        id: audioMenu
        Material.roundedScale: theme.radius
        Repeater {
            model: player.audioTracks.length
            delegate: MenuItem {
                text: page.trackName(player.audioTracks[index], index)
                checkable: true
                checked: player.activeAudioTrack === index
                onTriggered: player.activeAudioTrack = index
            }
        }
    }

    Shortcut { sequences: ["Space", "K"]; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.togglePlay() }
    Shortcut { sequence: "Left"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(-10000) }
    Shortcut { sequence: "Right"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(10000) }
    Shortcut { sequence: "J"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(-10000) }
    Shortcut { sequence: "L"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(10000) }
    Shortcut { sequence: "Shift+Left"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(-60000) }
    Shortcut { sequence: "Shift+Right"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(60000) }
    Shortcut { sequence: "Up"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: { audio.volume = Math.min(1, audio.volume + 0.05); osd.show(qsTr("Volume %1").arg(fmt.percent(audio.volume))) } }
    Shortcut { sequence: "Down"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: { audio.volume = Math.max(0, audio.volume - 0.05); osd.show(qsTr("Volume %1").arg(fmt.percent(audio.volume))) } }
    Shortcut { sequence: "M"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: { audio.muted = !audio.muted; osd.show(audio.muted ? qsTr("Muted") : qsTr("Sound on")) } }
    Shortcut { sequence: "F"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.toggleFullScreen() }
    Shortcut { sequence: "C"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.cycleSubtitles() }
    Shortcut { sequence: "S"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.cycleSubtitles() }
    Shortcut { sequence: "V"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: { if (library.openInMpv(page.media.url, player.position, page.pageTitle)) player.pause() } }
    Shortcut { sequence: "Home"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: player.position = 0 }
}
