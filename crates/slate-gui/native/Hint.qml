import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import Slate.Native

// Keep each hint anchored to its owner. Native shared tooltips can retain stale
// placement after a resize; a hint must never swallow a control's mouse click.
Basic.ToolTip {
    id: hint
    required property Item anchorItem
    parent: anchorItem
    font: anchorItem ? anchorItem.font : Qt.font({})
    delay: 500
    focus: false
    enabled: false
    closePolicy: Popup.NoAutoClose
    width: Math.min(implicitWidth, 420, Overlay.overlay ? Overlay.overlay.width - 16 : 420)
    x: anchorItem ? (anchorItem.width - width) / 2 : 0
    y: anchorItem && Overlay.overlay && anchorItem.mapToItem(Overlay.overlay, 0, 0).y + anchorItem.height + implicitHeight + 8 < Overlay.overlay.height ? anchorItem.height + 6 : -implicitHeight - 6
    background: Rectangle {
        radius: 4
        color: Theme.backgroundColor
        border.color: Qt.alpha(Theme.textColor, 0.3)
    }
    contentItem: Text {
        text: hint.text
        font: hint.font
        color: Theme.textColor
        wrapMode: Text.WrapAnywhere
    }
}
