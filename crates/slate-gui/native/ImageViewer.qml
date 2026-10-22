import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: viewer
    objectName: "imageViewer_" + paneId
    property int paneId: 0
    property var imageData: ({})
    property var bridge
    property var sendCommand
    property var invoke
    readonly property int document: imageData.document || 0
    property bool fit: true
    property real zoom: 1
    property bool restoring: true
    // Native dimensions are distinct from the bounded decoded texture size.
    property real naturalWidth: 0
    property real naturalHeight: 0
    readonly property real scaleFactor: fit
        ? Math.min(1, Math.max(1, area.width - 24) / Math.max(1, naturalWidth), Math.max(1, area.height - 24) / Math.max(1, naturalHeight))
        : zoom
    readonly property bool ready: picture.status === Image.Ready && !imageData.unavailable
    color: Theme.viewBackgroundColor
    Accessible.role: Accessible.Pane
    Accessible.name: "Image preview"
    Accessible.description: ready ? naturalWidth + " by " + naturalHeight + " pixels. " + imageData.path : "Image preview unavailable"
    activeFocusOnTab: true
    Keys.onPressed: function(event) {
        if (!ready || event.modifiers !== Qt.NoModifier) return;
        if (event.key === Qt.Key_Left) area.contentX = Math.max(0, area.contentX - 40);
        else if (event.key === Qt.Key_Right) area.contentX = Math.min(Math.max(0, area.contentWidth - area.width), area.contentX + 40);
        else if (event.key === Qt.Key_Up) area.contentY = Math.max(0, area.contentY - 40);
        else if (event.key === Qt.Key_Down) area.contentY = Math.min(Math.max(0, area.contentHeight - area.height), area.contentY + 40);
        else if (event.key === Qt.Key_PageUp) area.contentY = Math.max(0, area.contentY - area.height);
        else if (event.key === Qt.Key_PageDown) area.contentY = Math.min(Math.max(0, area.contentHeight - area.height), area.contentY + area.height);
        else return;
        event.accepted = true;
        persist();
    }

    function restore() {
        restoring = true;
        const state = imageData.viewport || ({});
        fit = state.fit !== false;
        zoom = state.zoom || 1;
        Qt.callLater(function() {
            area.contentX = Math.min(Math.max(0, area.contentWidth - area.width), state.x || 0);
            area.contentY = Math.min(Math.max(0, area.contentHeight - area.height), state.y || 0);
            restoring = false;
        });
    }
    function persist() {
        if (!restoring && document && sendCommand) sendCommand({
            "action": "image_viewport", "pane": paneId,
            "viewport": {"fit": fit, "zoom": zoom, "x": Math.max(0, area.contentX), "y": Math.max(0, area.contentY)}
        });
    }
    function setZoom(value) {
        zoom = Math.max(0.1, Math.min(8, value));
        fit = false;
        persist();
    }
    onDocumentChanged: restore()
    onImageDataChanged: {
        const state = imageData.viewport || ({});
        if (!restoring && (state.fit !== fit || state.zoom !== zoom)) restore();
    }
    Component.onCompleted: restore()
    TapHandler {
        onPressedChanged: if (pressed && viewer.sendCommand) viewer.sendCommand({"action": "focus", "pane": viewer.paneId})
    }
    ColumnLayout {
        anchors.fill: parent
        spacing: 0
        Flow {
            objectName: "imageToolbar_" + viewer.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.margins: 4
            spacing: 4
            Button {
                objectName: "imageFit_" + viewer.paneId
                text: "Fit"
                checkable: true
                checked: viewer.fit
                enabled: viewer.ready
                onClicked: { viewer.fit = true; area.contentX = 0; area.contentY = 0; viewer.persist(); }
            }
            Button {
                objectName: "imageActual_" + viewer.paneId
                text: "100%"
                enabled: viewer.ready
                onClicked: viewer.setZoom(1)
            }
            Button {
                objectName: "imageZoomOut_" + viewer.paneId
                text: "−"
                Accessible.name: "Zoom out"
                enabled: viewer.ready && viewer.scaleFactor > 0.1
                onClicked: viewer.setZoom(viewer.scaleFactor / 1.25)
            }
            Button {
                objectName: "imageZoomIn_" + viewer.paneId
                text: "+"
                Accessible.name: "Zoom in"
                enabled: viewer.ready && viewer.scaleFactor < 8
                onClicked: viewer.setZoom(viewer.scaleFactor * 1.25)
            }
            Button {
                objectName: "imageSource_" + viewer.paneId
                text: "Open as Text"
                visible: viewer.imageData.source_available === true
                onClicked: viewer.invoke("image-source")
            }
            Button {
                objectName: "imageReload_" + viewer.paneId
                text: "Reload"
                onClicked: viewer.invoke("reload")
            }
        }
        Label {
            objectName: "imageDetails_" + viewer.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.leftMargin: 6
            elide: Text.ElideRight
            textFormat: Text.PlainText
            text: viewer.ready ? viewer.naturalWidth + " × " + viewer.naturalHeight + " · "
                + Math.round(viewer.scaleFactor * 100) + "% · "
                + ((viewer.imageData.bytes || 0) / 1024).toFixed(1) + " KiB" : ""
        }
        Flickable {
            id: area
            objectName: "imagePan_" + viewer.paneId
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            contentWidth: Math.max(width, viewer.naturalWidth * viewer.scaleFactor)
            contentHeight: Math.max(height, viewer.naturalHeight * viewer.scaleFactor)
            onMovementEnded: viewer.persist()
            ScrollBar.horizontal: ScrollBar { onPressedChanged: if (!pressed) viewer.persist(); }
            ScrollBar.vertical: ScrollBar { onPressedChanged: if (!pressed) viewer.persist(); }
            Canvas {
                // A fixed viewport-sized pattern avoids a huge checkerboard at high zoom.
                width: area.width
                height: area.height
                x: area.contentX
                y: area.contentY
                onWidthChanged: requestPaint()
                onHeightChanged: requestPaint()
                onPaint: {
                    const ctx = getContext("2d");
                    ctx.fillStyle = "#dedede";
                    ctx.fillRect(0, 0, width, height);
                    ctx.fillStyle = "#bdbdbd";
                    for (let row = 0; row < height / 12; ++row)
                        for (let col = 0; col < width / 12; ++col)
                            if ((row + col) % 2) ctx.fillRect(col * 12, row * 12, 12, 12);
                }
            }
            Image {
                id: picture
                objectName: "imagePicture_" + viewer.paneId
                visible: viewer.ready
                source: viewer.document && !viewer.imageData.unavailable
                    ? viewer.bridge.imageSource(viewer.imageData.path, viewer.imageData.generation || 0) : ""
                asynchronous: true
                cache: false
                sourceSize: Qt.size(4096, 4096)
                width: viewer.naturalWidth * viewer.scaleFactor
                height: viewer.naturalHeight * viewer.scaleFactor
                x: Math.max(0, (area.contentWidth - width) / 2)
                y: Math.max(0, (area.contentHeight - height) / 2)
                fillMode: Image.PreserveAspectFit
                smooth: viewer.scaleFactor < 2
                onStatusChanged: if (status === Image.Ready) {
                    const nativeSize = viewer.bridge.imageSize(viewer.imageData.path, viewer.imageData.generation || 0);
                    viewer.naturalWidth = nativeSize.width;
                    viewer.naturalHeight = nativeSize.height;
                    Qt.callLater(viewer.restore);
                }
            }
            WheelHandler {
                target: null
                acceptedModifiers: Qt.ControlModifier
                onWheel: function(event) {
                    viewer.setZoom(viewer.scaleFactor * (event.angleDelta.y > 0 ? 1.25 : 0.8));
                    event.accepted = true;
                }
            }
            Label {
                objectName: "imageMessage_" + viewer.paneId
                x: area.contentX + 16
                y: area.contentY + Math.max(16, (area.height - height) / 2)
                width: Math.max(1, area.width - 32)
                padding: 12
                background: Rectangle { color: Theme.viewBackgroundColor; radius: 4 }
                textFormat: Text.PlainText
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
                visible: !viewer.ready
                text: viewer.imageData.unavailable ? "This image was deleted or moved. Reload after restoring the file."
                    : picture.status === Image.Loading ? "Loading image…"
                    : "Cannot display this image. It may be damaged, exceed image size limits, or need an image-format plugin."
            }
        }
    }
}
