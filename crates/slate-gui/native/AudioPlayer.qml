import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtMultimedia

// Audio tab: the core owns the transport state, this file owns the decoder.
// Qt Multimedia is optional, so this is a separate component that Main.qml
// loads lazily; if the module is missing the pane falls back to a message.
Rectangle {
    id: viewer
    objectName: "audioPlayer_" + paneId
    property int paneId: 0
    property var mediaData: ({})
    property var bridge
    property var sendCommand
    property var invoke
    // Last values reported to the core, so progress is not resent unchanged.
    property int reportedPosition: -1
    property int reportedDuration: -1
    property bool dragging: false

    readonly property int document: mediaData.document || 0
    readonly property bool ready: !!mediaData.path && !mediaData.unavailable
    readonly property bool playing: mediaData.playing === true
    readonly property int coreVolume: mediaData.volume === undefined ? 80 : mediaData.volume
    readonly property bool muted: mediaData.muted === true
    readonly property bool looping: mediaData.loop_playback === true
    readonly property real coreRate: mediaData.rate || 1
    readonly property int corePosition: mediaData.position_ms || 0
    readonly property int duration: Math.max(0, Math.round(player.duration || mediaData.duration_ms || 0))
    readonly property bool failed: !ready || player.error !== MediaPlayer.NoError || !!mediaData.error
    readonly property string errorText: {
        if (mediaData.error) return mediaData.error;
        if (player.error === MediaPlayer.NoError) return "";
        if (player.error === MediaPlayer.AccessDeniedError) return "Access denied while reading this file.";
        // Prefer Qt's own message: it names the missing device or codec.
        const reason = player.errorString || "Cannot play this file.";
        if (player.error === MediaPlayer.FormatError || player.error === MediaPlayer.ResourceError)
            return reason + " A decoder for this format may be missing (for GStreamer builds, install gstreamer1.0-libav).";
        return reason;
    }

    color: Theme.viewBackgroundColor
    Accessible.role: Accessible.Pane
    Accessible.name: "Audio player"
    Accessible.description: ready
        ? (mediaData.path || "") + (mediaData.muted ? ", muted" : ", volume " + coreVolume + "%")
        : "Audio unavailable"
    activeFocusOnTab: true

    function timeText(ms) {
        const total = Math.max(0, Math.round(ms / 1000));
        const seconds = total % 60;
        const minutes = Math.floor(total / 60);
        return minutes + ":" + (seconds < 10 ? "0" : "") + seconds;
    }
    function send(action) {
        if (viewer.document && viewer.sendCommand) viewer.sendCommand(action);
    }
    function control(value) {
        send({"action": "media_control", "pane": viewer.paneId, "control": value});
    }
    function report(ended) {
        if (!viewer.document || !viewer.sendCommand) return;
        const duration = Math.max(0, Math.round(player.duration || mediaData.duration_ms || 0));
        const position = ended ? 0 : Math.max(0, Math.round(player.position));
        const error = player.error === MediaPlayer.NoError ? null : viewer.errorText;
        if (!ended && !error && position === viewer.reportedPosition && duration === viewer.reportedDuration) return;
        viewer.reportedPosition = position;
        viewer.reportedDuration = duration;
        send({"action": "media_playback", "pane": viewer.paneId,
              "position_ms": position, "duration_ms": duration,
              "ended": ended, "error": error});
    }
    function playerPositionChanged() {
        if (!dragging && Math.abs(player.position - corePosition) > 1000) report(false);
    }
    function syncPlayback() {
        if (!ready) return;
        if (playing && player.playbackState !== MediaPlayer.PlayingState) player.play();
        else if (!playing && player.playbackState === MediaPlayer.PlayingState) player.pause();
    }
    function seekTo(ms) {
        const target = Math.max(0, Math.min(duration, ms));
        player.position = target;
        send({"action": "media_seek", "pane": viewer.paneId, "position_ms": Math.round(target)});
    }
    function setVolume(value) {
        send({"action": "media_volume", "pane": viewer.paneId,
              "volume": Math.max(0, Math.min(100, Math.round(value)))});
    }

    Keys.onPressed: function(event) {
        if (!ready) return;
        if (event.key === Qt.Key_Space) control(playing ? "pause" : "play");
        else if (event.key === Qt.Key_Left) seekTo(player.position - 5000);
        else if (event.key === Qt.Key_Right) seekTo(player.position + 5000);
        else if (event.key === Qt.Key_Home) seekTo(0);
        else if (event.key === Qt.Key_M) send({"action": "media_mute", "pane": viewer.paneId});
        else if (event.key === Qt.Key_L) send({"action": "media_loop", "pane": viewer.paneId});
        else if (event.key === Qt.Key_Up) setVolume(coreVolume + 5);
        else if (event.key === Qt.Key_Down) setVolume(coreVolume - 5);
        else return;
        event.accepted = true;
    }

    TapHandler {
        onPressedChanged: if (pressed) {
            viewer.forceActiveFocus();
            if (viewer.sendCommand) viewer.sendCommand({"action": "focus", "pane": viewer.paneId});
        }
    }
    MediaPlayer {
        id: player
        objectName: "audioMedia_" + viewer.paneId
        source: viewer.ready ? viewer.bridge.fileUrl(viewer.mediaData.path) : ""
        playbackRate: viewer.coreRate
        loops: viewer.looping ? MediaPlayer.Infinite : 1
        audioOutput: AudioOutput {
            objectName: "audioOutput_" + viewer.paneId
            volume: viewer.muted ? 0 : viewer.coreVolume / 100
            muted: viewer.muted
        }
        onErrorOccurred: viewer.report(false)
        onPositionChanged: viewer.playerPositionChanged()
        onMediaStatusChanged: {
            if (mediaStatus === MediaPlayer.EndOfMedia && !viewer.looping) viewer.report(true);
        }
        onDurationChanged: viewer.report(false)
    }
    onPlayingChanged: syncPlayback()
    onMediaDataChanged: {
        // Follow a stop, a recovery or a seek this player did not make. A core
        // position that equals the last report is our own progress coming back,
        // so it must never drag the playhead backwards.
        const echo = corePosition === reportedPosition;
        if (!dragging && ready && !echo && Math.abs(player.position - corePosition) > 300)
            player.position = corePosition;
        syncPlayback();
    }
    Component.onCompleted: {
        if (ready) {
            player.position = corePosition;
            syncPlayback();
        }
    }
    Timer {
        interval: 250
        repeat: true
        running: viewer.playing && viewer.ready
        onTriggered: viewer.report(false)
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0
        Flow {
            objectName: "audioToolbar_" + viewer.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.margins: 4
            spacing: 4
            Button {
                objectName: "audioPlay_" + viewer.paneId
                text: viewer.playing ? "Pause" : "Play"
                enabled: viewer.ready
                onClicked: viewer.control(viewer.playing ? "pause" : "play")
            }
            Button {
                objectName: "audioStop_" + viewer.paneId
                text: "Stop"
                enabled: viewer.ready && (viewer.playing || player.position > 0)
                onClicked: viewer.control("stop")
            }
            Button {
                objectName: "audioMute_" + viewer.paneId
                text: "Mute"
                checkable: true
                checked: viewer.muted
                enabled: viewer.ready
                onClicked: viewer.send({"action": "media_mute", "pane": viewer.paneId, "muted": checked})
            }
            Button {
                objectName: "audioLoop_" + viewer.paneId
                text: "Loop"
                checkable: true
                checked: viewer.looping
                enabled: viewer.ready
                onClicked: viewer.send({"action": "media_loop", "pane": viewer.paneId, "loop_playback": checked})
            }
            ComboBox {
                objectName: "audioRate_" + viewer.paneId
                implicitWidth: 84
                enabled: viewer.ready
                model: ["0.5×", "0.75×", "1×", "1.25×", "1.5×", "2×"]
                readonly property var rates: [0.5, 0.75, 1, 1.25, 1.5, 2]
                currentIndex: Math.max(0, rates.indexOf(viewer.coreRate) === -1 ? 2 : rates.indexOf(viewer.coreRate))
                onActivated: function(index) {
                    viewer.send({"action": "media_rate", "pane": viewer.paneId, "rate": rates[index]});
                }
            }
            Button {
                objectName: "audioReload_" + viewer.paneId
                text: "Reload"
                onClicked: viewer.invoke("reload")
            }
        }
        Label {
            objectName: "audioDetails_" + viewer.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.leftMargin: 6
            elide: Text.ElideRight
            textFormat: Text.PlainText
            text: viewer.ready
                ? (viewer.mediaData.path || "").replace(/^.*[\\\/]/, "") + " · "
                    + ((viewer.mediaData.bytes || 0) / 1024).toFixed(1) + " KiB · "
                    + (viewer.duration > 0 ? viewer.timeText(viewer.duration) : "duration unknown")
                : ""
        }
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 6
            Layout.rightMargin: 6
            spacing: 6
            Label {
                objectName: "audioPosition_" + viewer.paneId
                text: viewer.timeText(viewer.dragging ? seek.value : player.position)
                font.family: "monospace"
            }
            Slider {
                id: seek
                objectName: "audioSeek_" + viewer.paneId
                Layout.fillWidth: true
                enabled: viewer.ready && viewer.duration > 0
                from: 0
                to: Math.max(1, viewer.duration)
                value: Math.min(to, player.position)
                onPressedChanged: {
                    viewer.dragging = pressed;
                    if (!pressed) viewer.seekTo(value);
                }
                onMoved: if (viewer.dragging) player.position = value
            }
            Label {
                objectName: "audioDuration_" + viewer.paneId
                text: viewer.duration > 0 ? viewer.timeText(viewer.duration) : "--:--"
                font.family: "monospace"
            }
            Button {
                objectName: "audioQuieter_" + viewer.paneId
                text: "−"
                Accessible.name: "Quieter"
                enabled: viewer.ready && viewer.coreVolume > 0
                onClicked: viewer.setVolume(viewer.coreVolume - 5)
            }
            Label {
                objectName: "audioVolume_" + viewer.paneId
                text: viewer.muted ? "muted" : viewer.coreVolume + "%"
                horizontalAlignment: Text.AlignRight
                Layout.minimumWidth: 44
            }
            Button {
                objectName: "audioLouder_" + viewer.paneId
                text: "+"
                Accessible.name: "Louder"
                enabled: viewer.ready && viewer.coreVolume < 100
                onClicked: viewer.setVolume(viewer.coreVolume + 5)
            }
        }
        Label {
            objectName: "audioMessage_" + viewer.paneId
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: 16
            verticalAlignment: Text.AlignVCenter
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            textFormat: Text.PlainText
            color: Theme.disabledTextColor
            text: !viewer.ready
                ? (viewer.mediaData.unavailable
                    ? "This audio file was deleted or moved. Reload after restoring it."
                    : "No audio file is open.")
                : viewer.errorText !== "" ? viewer.errorText : ""
            visible: text !== ""
        }
        Item { Layout.fillHeight: true }
    }
}
