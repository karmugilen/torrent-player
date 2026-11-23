import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import TorrentPlayer

Page {
    id: page
    readonly property bool isPlayer: true
    property var media: ({})
    readonly property string torrentKey: media.key || ""
    readonly property string pageTitle: media.title || ""
    readonly property bool active: StackView.status === StackView.Active && !win.dialogOpen
    property bool controlsVisible: true
    property bool thumbnailTaken: false
    property bool left_: false
    property int subtitleChoice: -1      // torrent subtitle index, -2 a picked file, -1 none
    property string subtitleLabel: ""
    property string pendingSubtitlePath: ""
    property bool holdFast: false        // Right held: 2x until released
    readonly property var torrent: library.revision >= 0 ? library.details(torrentKey) : ({})
    readonly property var embeddedSubtitles: player.subtitleTracks.filter(t => !t.external)
    // The episode run (natural order, extras left out) and where this file is in it.
    readonly property var playlist: media.playlist || []
    readonly property int playlistPos: playlist.findIndex(item => item.index === media.fileIndex)
    readonly property bool hasPrevious: playlistPos > 0
    readonly property bool hasNext: playlistPos >= 0 && playlistPos < playlist.length - 1
    readonly property bool playNext: media.playNext !== false
    property int upNext: -1              // playlist place of the "Up next" card, -1 hidden
    property int upNextLeft: 0
    property bool endHandled: false
    property bool switching: false
    // After release/arrow seek, hold the bar at the target until mpv gets there (Android pendingSeek).
    property real pendingSeekMs: -1
    readonly property var playingFile: (torrent.files || []).find(f => f.index === media.fileIndex) || ({})
    // Seek bar and clock: drag target, else pending seek, else mpv position.
    readonly property real displayMs: playback.shownPlayheadMs(seekBar.dragging, seekBar.targetMs, pendingSeekMs, player.position)
    readonly property bool seekSpinning: pendingSeekMs >= 0 && !seekBar.dragging
    background: Rectangle { color: "black" }

    function showControls() {
        if (win.inPip) return
        controlsVisible = true
        hideTimer.restart()
    }
    function togglePip() {
        if (!pipWindow) return
        if (win.inPip) {
            pipWindow.leave()
            showControls()
            return
        }
        if (win.visibility === Window.FullScreen)
            win.showNormal()
        subtitleMenu.close()
        audioMenu.close()
        controlsVisible = false
        pipWindow.enter(win, player.videoAspect || (16 / 9))
    }
    function clampMs(ms) { return Math.max(0, player.duration > 0 ? Math.min(player.duration, ms) : ms) }
    function clearPendingSeek() { pendingSeekMs = -1; pendingSeekClear.stop() }
    function maybeClearPendingSeek() {
        if (playback.pendingSeekCaughtUp(pendingSeekMs, player.position, player.seeking))
            clearPendingSeek()
    }
    function beginPendingSeek(target) {
        pendingSeekMs = target
        pendingSeekClear.restart()
    }
    // Arrow skips: ask the engine for the spot first, as a seek-bar release does.
    function seekBy(ms) {
        const target = clampMs(player.position + ms)
        beginPendingSeek(target)
        playback.prefetchNow(target)
        player.seek(target, false)
        showControls()
        osd.show((ms > 0 ? "+" : "−") + Math.abs(ms / 1000) + " s")
    }
    function seekTo(ms) {
        const target = clampMs(ms)
        beginPendingSeek(target)
        playback.prefetchNow(target)
        player.seek(target, false)
        showControls()
    }
    function togglePlay() {
        player.paused = !player.paused
        showControls()
    }
    function toggleFullScreen() {
        if (win.visibility === Window.FullScreen) win.showNormal()
        else win.showFullScreen()
    }
    function changeVolume(step) {
        player.volume = Math.max(0, Math.min(1, player.volume + step))
        player.muted = false
        osd.show(qsTr("Volume %1").arg(fmt.percent(player.volume)))
    }
    function toggleMute() {
        player.muted = !player.muted
        osd.show(player.muted ? qsTr("Muted") : qsTr("Sound on"))
    }
    function openInMpv() {
        if (library.openInMpv(page.media.url, player.position, page.pageTitle))
            player.paused = true
    }
    // An external subtitle mpv already has is selected again, not added twice.
    function selectExternal(path, label) {
        const known = player.subtitleTracks.find(t => t.external && t.externalFilename === path)
        if (known) player.subtitleId = known.id
        else player.addSubtitle(path, label)
    }
    function loadSubtitleFile(path) {
        pendingSubtitle.stop()
        subtitleChoice = -2
        subtitleLabel = path.split("/").pop()
        selectExternal(path, subtitleLabel)
        osd.show(subtitleLabel)
    }
    function chooseTorrentSubtitle(sub) {
        pendingSubtitle.stop()
        subtitleChoice = sub.index
        subtitleLabel = sub.label
        const path = sub.ready ? sub.path : library.finishedFilePath(torrentKey, sub.index)
        if (path !== "") {
            selectExternal(path, sub.label)
        } else {
            // Not downloaded yet: fetch it (it's small) and load it when it lands.
            library.setFileSelected(torrentKey, sub.index, true)
            player.subtitleId = -1
            pendingSubtitle.restart()
            osd.show(qsTr("Downloading %1…").arg(sub.label))
        }
    }
    function chooseEmbedded(track) {
        pendingSubtitle.stop()
        subtitleChoice = -1
        subtitleLabel = ""
        player.subtitleId = track.id
    }
    function subtitlesOff() {
        pendingSubtitle.stop()
        subtitleChoice = -1
        subtitleLabel = ""
        player.subtitleId = -1
    }
    function currentSubtitleName() {
        if (subtitleChoice !== -1) return subtitleLabel
        const t = player.subtitleTracks.find(t => t.id === player.subtitleId)
        return t ? t.label : ""
    }
    function cycleSubtitles() {
        // Off → each torrent subtitle → each embedded track → off.
        const list = media.subtitles || []
        let order = [{ kind: "off" }]
        for (const s of list) order.push({ kind: "file", sub: s })
        for (const t of embeddedSubtitles) order.push({ kind: "track", track: t })
        let current = 0
        for (let i = 0; i < order.length; ++i) {
            const o = order[i]
            if ((o.kind === "file" && o.sub.index === subtitleChoice) ||
                (o.kind === "track" && subtitleChoice === -1 && o.track.id === player.subtitleId))
                current = i
        }
        const next = order[(current + 1) % order.length]
        if (next.kind === "off") { subtitlesOff(); osd.show(qsTr("Subtitles off")) }
        else if (next.kind === "file") { chooseTorrentSubtitle(next.sub); osd.show(next.sub.label) }
        else { chooseEmbedded(next.track); osd.show(next.track.label) }
    }
    // Another episode in this player (Android switchTo): this one is saved as the page closes.
    function switchEpisode(pos) {
        if (pos < 0 || pos >= playlist.length || switching) return
        upNext = -1
        switching = true
        osd.show(playlist[pos].title)
        library.play(torrentKey, playlist[pos].index)
        switchReset.restart()
    }
    function nextEpisode() { if (hasNext) switchEpisode(playlistPos + 1) }
    function previousEpisode() { if (hasPrevious) switchEpisode(playlistPos - 1) }
    // End of an episode: kept as watched, then the Up next card, or close after the last one.
    function onEpisodeEnd() {
        if (endHandled || player.duration <= 0) return
        endHandled = true
        if (!hasNext) {
            page.StackView.view.pop()
            return
        }
        upNextLeft = Math.max(1, media.playNextCountdownSeconds || 5)
        upNext = playlistPos + 1
    }
    function leave() {
        if (left_) return
        left_ = true
        // Near the end counts as watched: the full length is saved, which the library keeps
        // as place 0 and marks watched, so Continue moves on to the next episode.
        const nearEnd = player.duration > 0 && (player.eof || player.position > player.duration * 0.95)
        library.savePosition(torrentKey, media.fileIndex, nearEnd ? player.duration : player.position,
                             player.duration > 0 ? player.duration : -1)
        player.speed = 1.0
        player.stop()
        if (pipWindow && win.inPip)
            pipWindow.leave()
        if (win.visibility === Window.FullScreen)
            win.showNormal()
    }

    StackView.onRemoved: leave()
    Component.onDestruction: leave()
    onActiveChanged: if (active) keyCatcher.forceActiveFocus()
    onMediaChanged: clearPendingSeek()

    readonly property var engineHandle: engine
    PlaybackControl {
        id: playback
        engine: page.engineHandle
        media: page.media
        durationMs: player.duration
    }

    MpvPlayer {
        id: player
        anchors.fill: parent
        startMs: (page.media.startMs || 0) > 5000 ? page.media.startMs : 0
        source: page.media.url || ""
        onFileLoaded: if (startMs > 0) osd.show(qsTr("Resumed at %1").arg(fmt.clock(startMs)))
        onScreenshotSaved: (path, ok) => { if (!ok) page.thumbnailTaken = false }
        onNotice: (text) => win.toast(text)
        onEofChanged: {
            if (eof) page.onEpisodeEnd()
            else { page.endHandled = false; page.upNext = -1 }
        }
    }
    PlayerStats { id: stats }
    // Play next on: the card counts down, then the next episode plays.
    Timer {
        interval: 1000
        repeat: true
        running: page.upNext >= 0 && page.playNext
        onTriggered: {
            page.upNextLeft -= 1
            if (page.upNextLeft <= 0) page.switchEpisode(page.upNext)
        }
    }
    // library.play gave nothing (an error message instead): allow another try.
    Timer { id: switchReset; interval: 10000; onTriggered: page.switching = false }
    // Drop the held seek target if mpv never reaches it (same 4 s as Android).
    Timer {
        id: pendingSeekClear
        interval: 4000
        onTriggered: page.clearPendingSeek()
    }
    Connections {
        target: player
        function onPositionChanged() { page.maybeClearPendingSeek() }
        function onBufferingChanged() { page.maybeClearPendingSeek() }
    }

    // A frame for the home card, once real picture is on screen.
    Timer {
        interval: 8000
        running: !player.paused && !player.buffering && player.loaded && !page.thumbnailTaken && !page.torrent.thumbnail
        onTriggered: { page.thumbnailTaken = true; player.screenshotTo(library.thumbnailPath(page.torrentKey)) }
    }
    Timer {
        id: pendingSubtitle
        interval: 1000
        repeat: true
        onTriggered: {
            const path = library.finishedFilePath(page.torrentKey, page.subtitleChoice)
            if (path !== "") {
                stop()
                page.selectExternal(path, page.subtitleLabel)
            }
        }
    }

    // Keys: Right is here, not a Shortcut, because holding it plays at 2x until it is let go.
    Item {
        id: keyCatcher
        focus: true
        Component.onCompleted: forceActiveFocus()
        Keys.onPressed: (event) => {
            if (event.key !== Qt.Key_Right || event.modifiers !== Qt.NoModifier || !page.active) return
            event.accepted = true
            if (event.isAutoRepeat && !page.holdFast) {
                page.holdFast = true
                player.speed = 2.0
            }
        }
        Keys.onReleased: (event) => {
            if (event.key !== Qt.Key_Right || event.modifiers !== Qt.NoModifier || !page.active) return
            event.accepted = true
            if (event.isAutoRepeat) return
            if (page.holdFast) { page.holdFast = false; player.speed = 1.0 }
            else page.seekBy(10000)
        }
    }

    // Waiting for data, for the file to open, or for a seek (including pending outside-buffer seeks).
    BusyIndicator {
        anchors.centerIn: parent
        running: player.errorString === "" && (player.buffering || page.seekSpinning)
        visible: running
    }
    Label {
        anchors { horizontalCenter: parent.horizontalCenter; top: parent.verticalCenter; topMargin: 48 }
        visible: player.errorString === "" && (player.buffering || page.seekSpinning) && !playback.complete
        text: qsTr("Buffering · %1 · %n peers", "", playback.peers).arg(fmt.speed(playback.downloadSpeed))
        color: "#cccccc"
    }
    Rectangle {
        id: errorBox
        anchors.centerIn: parent
        visible: player.errorString !== ""
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
            Label { Layout.fillWidth: true; text: player.errorString; color: win.muted; wrapMode: Text.Wrap }
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

    // Info panel (I): Android's stats line, the file's progress and a compact piece map.
    Rectangle {
        id: infoPanel
        anchors { left: parent.left; top: parent.top; leftMargin: 16; topMargin: 72 }
        visible: stats.shown && !win.inPip
        z: 2
        width: 340
        height: infoColumn.implicitHeight + 20
        radius: win.radius
        color: Qt.rgba(0, 0, 0, 0.72)
        ColumnLayout {
            id: infoColumn
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 10 }
            spacing: 4
            Label {
                Layout.fillWidth: true
                text: stats.line(page.media.directPlay === true,
                                 page.playingFile.progress !== undefined ? page.playingFile.progress : -1,
                                 playback.peers, playback.downloadSpeed)
                color: "white"
                font.pixelSize: 12
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: stats.buffered(player.position, player.cacheEndMs)
                color: "#cccccc"
                font.pixelSize: 12
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: stats.fileLine(page.playingFile.progress !== undefined ? page.playingFile.progress : -1,
                                     page.playingFile.length || 0, page.torrent.uploadSpeed || 0)
                color: "#cccccc"
                font.pixelSize: 12
            }
            Label {
                Layout.topMargin: 4
                visible: playerPieces.hasMap
                text: (page.media.directPlay ? qsTr("RAM pieces") : qsTr("Pieces")) + " · " + (playerPieces.summary.titlePrimary || "")
                color: "#cccccc"
                font.pixelSize: 11
            }
            PieceGrid {
                id: playerPieces
                Layout.fillWidth: true
                Layout.preferredHeight: implicitHeight
                engine: page.engineHandle
                torrentId: page.media.engineId || ""
                active: stats.shown && page.active
                frozen: page.torrent.paused === true
                memoryBuffer: page.media.directPlay === true
                doneColor: theme.accent
                outlineColor: "#9d9da6"
                onSurfaceColor: "white"
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: playerPieces.summary.bufferLine || ""
                color: "#cccccc"
                font.pixelSize: 11
            }
        }
    }

    // Up next: the following episode starts after the countdown (Play next on); Cancel stays here.
    Rectangle {
        id: upNextCard
        anchors.centerIn: parent
        visible: page.upNext >= 0 && !win.inPip
        width: Math.max(280, upNextColumn.implicitWidth + 40)
        height: upNextColumn.implicitHeight + 28
        radius: 16
        color: Qt.rgba(0, 0, 0, 0.75)
        z: 2
        ColumnLayout {
            id: upNextColumn
            anchors.centerIn: parent
            spacing: 6
            Label {
                Layout.alignment: Qt.AlignHCenter
                text: page.playNext ? qsTr("Up next in %1").arg(page.upNextLeft) : qsTr("Up next")
                color: Qt.rgba(1, 1, 1, 0.7)
                font.pixelSize: 13
            }
            Label {
                Layout.alignment: Qt.AlignHCenter
                Layout.maximumWidth: 420
                text: page.upNext >= 0 && page.playlist[page.upNext] ? page.playlist[page.upNext].title : ""
                color: "white"
                font.pixelSize: 17
                elide: Text.ElideRight
            }
            RowLayout {
                Layout.alignment: Qt.AlignHCenter
                Layout.topMargin: 6
                spacing: 8
                Button { text: qsTr("Cancel"); flat: true; focusPolicy: Qt.NoFocus; onClicked: { page.upNext = -1; page.showControls() } }
                Button { text: qsTr("⏭ Play now"); highlighted: true; focusPolicy: Qt.NoFocus; onClicked: page.switchEpisode(page.upNext) }
            }
        }
    }

    // Holding Right: 2x, as long as it is held.
    Rectangle {
        anchors { top: parent.top; horizontalCenter: parent.horizontalCenter; topMargin: 24 }
        visible: page.holdFast
        width: fastLabel.implicitWidth + 24
        height: fastLabel.implicitHeight + 10
        radius: height / 2
        color: Qt.rgba(0, 0, 0, 0.6)
        Label { id: fastLabel; anchors.centerIn: parent; text: qsTr("2x ▸▸"); color: "white"; font.pixelSize: 14 }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: win.inPip || controlsVisible ? Qt.ArrowCursor : Qt.BlankCursor
        acceptedButtons: Qt.LeftButton
        onPositionChanged: page.showControls()
        onPressed: keyCatcher.forceActiveFocus()
        onClicked: {
            if (win.inPip) page.togglePip()
            else page.togglePlay()
        }
        onDoubleClicked: {
            if (win.inPip) return
            page.togglePlay()
            page.toggleFullScreen()
        }
        onWheel: (wheel) => { if (!win.inPip) page.changeVolume(wheel.angleDelta.y > 0 ? 0.05 : -0.05) }
    }
    Timer {
        id: hideTimer
        interval: 3000
        running: true
        onTriggered: if (!subtitleMenu.visible && !audioMenu.visible && !seekBar.dragging && !controlsHover.hovered) page.controlsVisible = false
    }

    // Top bar.
    Rectangle {
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 64
        opacity: controlsVisible && !win.inPip ? 1 : 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 200 } }
        gradient: Gradient {
            GradientStop { position: 0; color: Qt.rgba(0, 0, 0, 0.75) }
            GradientStop { position: 1; color: "transparent" }
        }
        RowLayout {
            anchors { fill: parent; leftMargin: 12; rightMargin: 16 }
            ToolButton { text: "←"; font.pixelSize: 20; focusPolicy: Qt.NoFocus; onClicked: page.StackView.view.pop() }
            Label { Layout.fillWidth: true; text: page.playlist.length > 1 && page.media.episode ? page.media.episode : page.pageTitle; color: "white"; elide: Text.ElideMiddle; font.pixelSize: 16 }
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
        id: bottomBar
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 104
        opacity: controlsVisible && !win.inPip ? 1 : 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 200 } }
        gradient: Gradient {
            GradientStop { position: 0; color: "transparent" }
            GradientStop { position: 1; color: Qt.rgba(0, 0, 0, 0.85) }
        }
        HoverHandler { id: controlsHover }
        ColumnLayout {
            anchors { fill: parent; leftMargin: 20; rightMargin: 20; bottomMargin: 8; topMargin: 20 }
            spacing: 0
            RowLayout {
                Layout.fillWidth: true
                spacing: 12
                Label { text: fmt.clock(page.displayMs); color: "white"; font.pixelSize: 13 }

                // Seek bar as Android's: downloaded parts, mpv's read-ahead, and a drag that
                // seeks live over ready spots and only prefetches the others until release.
                Item {
                    id: seekBar
                    Layout.fillWidth: true
                    Layout.preferredHeight: 28
                    property bool dragging: false
                    property real originMs: 0
                    property real targetMs: 0
                    property string bubble: ""
                    property string note: ""
                    readonly property real span: Math.max(1, player.duration)
                    function msAt(x) { return Math.max(0, Math.min(1, x / width)) * player.duration }
                    function drag(x) {
                        targetMs = msAt(x)
                        const step = playback.scrub(targetMs, originMs, player.position, player.cacheEndMs, player.seeking)
                        bubble = step.label
                        note = step.note
                        if (step.action === "seek") player.seek(targetMs, false)
                        page.showControls()
                    }

                    Rectangle {
                        id: track
                        anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter }
                        height: seekArea.containsMouse || seekBar.dragging ? 6 : 4
                        radius: height / 2
                        color: Qt.rgba(1, 1, 1, 0.18)
                        Repeater {
                            model: playback.readyRanges
                            delegate: Rectangle {
                                x: modelData.from * track.width
                                width: Math.max(1, (modelData.to - modelData.from) * track.width)
                                height: track.height
                                radius: track.radius
                                color: Qt.rgba(1, 1, 1, 0.32)
                            }
                        }
                        Rectangle {
                            // mpv's cache ahead of the playhead.
                            x: player.position / seekBar.span * track.width
                            width: Math.max(0, Math.min(track.width - x, (player.cacheEndMs - player.position) / seekBar.span * track.width))
                            height: track.height
                            radius: track.radius
                            color: Qt.rgba(1, 1, 1, 0.55)
                        }
                        Rectangle {
                            width: Math.min(1, page.displayMs / seekBar.span) * track.width
                            height: track.height
                            radius: track.radius
                            color: theme.accent
                        }
                    }
                    Rectangle {
                        width: 14; height: 14; radius: 7
                        color: "white"
                        anchors.verticalCenter: parent.verticalCenter
                        x: Math.min(1, page.displayMs / seekBar.span) * seekBar.width - width / 2
                        visible: player.duration > 0
                    }
                    Rectangle {
                        id: bubbleBox
                        visible: seekBar.dragging && seekBar.bubble !== ""
                        y: -height - 10
                        x: Math.max(-20, Math.min(seekBar.width - width + 20, seekBar.targetMs / seekBar.span * seekBar.width - width / 2))
                        width: bubbleColumn.implicitWidth + 20
                        height: bubbleColumn.implicitHeight + 10
                        radius: win.smallRadius
                        color: Qt.rgba(0, 0, 0, 0.8)
                        Column {
                            id: bubbleColumn
                            anchors.centerIn: parent
                            Label { anchors.horizontalCenter: parent.horizontalCenter; text: seekBar.bubble; color: "white"; font.pixelSize: 14 }
                            Label { anchors.horizontalCenter: parent.horizontalCenter; visible: text !== ""; text: seekBar.note; color: "#ffcc66"; font.pixelSize: 12 }
                        }
                    }
                    MouseArea {
                        id: seekArea
                        anchors.fill: parent
                        hoverEnabled: true
                        enabled: player.duration > 0
                        onPressed: (mouse) => {
                            seekBar.dragging = true
                            seekBar.originMs = player.position
                            seekBar.drag(mouse.x)
                        }
                        onPositionChanged: (mouse) => { if (pressed) seekBar.drag(mouse.x) }
                        onReleased: (mouse) => {
                            // Recompute from release x (a click has no move). seekTo sets
                            // pendingSeek before dragging clears so the bar does not snap
                            // back to demuxer-cache-time when the spot is not ready yet.
                            seekBar.targetMs = seekBar.msAt(mouse.x)
                            seekBar.bubble = ""
                            seekBar.note = ""
                            page.seekTo(seekBar.targetMs)
                            seekBar.dragging = false
                            keyCatcher.forceActiveFocus()
                        }
                        onCanceled: {
                            seekBar.dragging = false
                            seekBar.bubble = ""
                            seekBar.note = ""
                        }
                    }
                }
                Label { text: fmt.clock(player.duration); color: "white"; font.pixelSize: 13 }
                Label {
                    visible: text !== ""
                    text: stats.buffered(player.position, player.cacheEndMs)
                    color: Qt.rgba(1, 1, 1, 0.55)
                    font.pixelSize: 12
                }
            }
            RowLayout {
                Layout.fillWidth: true
                spacing: 4
                // A series adds Previous and Next episode on both sides.
                ToolButton {
                    visible: page.playlist.length > 1
                    enabled: page.hasPrevious
                    text: "⏮"
                    font.pixelSize: 16
                    focusPolicy: Qt.NoFocus
                    ToolTip.text: qsTr("Previous episode (Shift+N)")
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    onClicked: page.previousEpisode()
                }
                ToolButton { text: "⟲ 10"; font.pixelSize: 13; focusPolicy: Qt.NoFocus; onClicked: page.seekBy(-10000) }
                ToolButton {
                    text: player.paused ? "▶" : "❚❚"
                    font.pixelSize: 18
                    focusPolicy: Qt.NoFocus
                    onClicked: page.togglePlay()
                }
                ToolButton { text: "10 ⟳"; font.pixelSize: 13; focusPolicy: Qt.NoFocus; onClicked: page.seekBy(10000) }
                ToolButton {
                    visible: page.playlist.length > 1
                    enabled: page.hasNext
                    text: "⏭"
                    font.pixelSize: 16
                    focusPolicy: Qt.NoFocus
                    ToolTip.text: qsTr("Next episode (N)")
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    onClicked: page.nextEpisode()
                }
                ToolButton {
                    text: player.muted || player.volume === 0 ? qsTr("Muted") : qsTr("Volume")
                    font.pixelSize: 13
                    focusPolicy: Qt.NoFocus
                    onClicked: page.toggleMute()
                }
                Slider {
                    Layout.preferredWidth: 110
                    from: 0; to: 1
                    focusPolicy: Qt.NoFocus
                    value: player.volume
                    onMoved: { player.volume = value; player.muted = false }
                }
                Item { Layout.fillWidth: true }
                ToolButton {
                    text: page.currentSubtitleName() !== "" ? qsTr("Subtitles: %1").arg(page.currentSubtitleName()) : qsTr("Subtitles")
                    font.pixelSize: 13
                    focusPolicy: Qt.NoFocus
                    onClicked: subtitleMenu.popup()
                    Layout.maximumWidth: 280
                }
                ToolButton {
                    visible: player.audioTracks.length > 1
                    text: qsTr("Audio")
                    font.pixelSize: 13
                    focusPolicy: Qt.NoFocus
                    onClicked: audioMenu.popup()
                }
                ToolButton {
                    text: qsTr("Open in mpv")
                    font.pixelSize: 13
                    focusPolicy: Qt.NoFocus
                    onClicked: page.openInMpv()
                }
                ToolButton {
                    text: qsTr("PiP")
                    font.pixelSize: 13
                    focusPolicy: Qt.NoFocus
                    ToolTip.text: qsTr("Picture-in-picture (P)")
                    ToolTip.visible: hovered
                    ToolTip.delay: 600
                    onClicked: page.togglePip()
                }
                ToolButton {
                    text: win.visibility === Window.FullScreen ? qsTr("Exit full screen") : qsTr("Full screen")
                    font.pixelSize: 13
                    focusPolicy: Qt.NoFocus
                    onClicked: page.toggleFullScreen()
                }
            }
        }
    }

    Menu {
        id: subtitleMenu
        Material.roundedScale: theme.radius
        onClosed: keyCatcher.forceActiveFocus()
        MenuItem { text: qsTr("Off"); checkable: true; checked: page.subtitleChoice === -1 && player.subtitleId < 0; onTriggered: page.subtitlesOff() }
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
            model: page.embeddedSubtitles
            delegate: MenuItem {
                text: qsTr("Embedded: %1").arg(modelData.label)
                checkable: true
                checked: page.subtitleChoice === -1 && player.subtitleId === modelData.id
                onTriggered: page.chooseEmbedded(modelData)
            }
        }
        MenuSeparator {}
        MenuItem { text: qsTr("Open subtitle file…"); onTriggered: picker.open("subtitle") }
    }
    Menu {
        id: audioMenu
        Material.roundedScale: theme.radius
        onClosed: keyCatcher.forceActiveFocus()
        Repeater {
            model: player.audioTracks
            delegate: MenuItem {
                text: modelData.label
                checkable: true
                checked: player.audioId === modelData.id
                onTriggered: player.audioId = modelData.id
            }
        }
    }

    Shortcut { sequences: ["Space", "K"]; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.togglePlay() }
    Shortcut { sequence: "Left"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(-10000) }
    // Right normally goes to keyCatcher (tap skips, hold plays 2x); this covers it losing focus.
    Shortcut { sequence: "Right"; context: Qt.ApplicationShortcut; enabled: page.active && !keyCatcher.activeFocus; onActivated: page.seekBy(10000) }
    Shortcut { sequence: "J"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(-10000) }
    Shortcut { sequence: "L"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(10000) }
    Shortcut { sequence: "Shift+Left"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(-60000) }
    Shortcut { sequence: "Shift+Right"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekBy(60000) }
    Shortcut { sequence: "Up"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.changeVolume(0.05) }
    Shortcut { sequence: "Down"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.changeVolume(-0.05) }
    Shortcut { sequence: "M"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.toggleMute() }
    Shortcut { sequence: "F"; context: Qt.ApplicationShortcut; enabled: page.active && !win.inPip; onActivated: page.toggleFullScreen() }
    Shortcut { sequence: "P"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.togglePip() }
    Shortcut { sequence: "C"; context: Qt.ApplicationShortcut; enabled: page.active && !win.inPip; onActivated: page.cycleSubtitles() }
    Shortcut { sequence: "S"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.cycleSubtitles() }
    Shortcut { sequence: "V"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.openInMpv() }
    Shortcut { sequence: "."; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: player.frameStep() }
    Shortcut { sequence: "N"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.nextEpisode() }
    Shortcut { sequence: "Shift+N"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.previousEpisode() }
    Shortcut { sequence: "I"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: stats.shown = !stats.shown }
    Shortcut { sequence: "Home"; context: Qt.ApplicationShortcut; enabled: page.active; onActivated: page.seekTo(0) }
}
