import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import org.kde.kirigami as Kirigami

// Keep each hint anchored to its owner. Native shared tooltips can retain stale
// placement after a resize; a hint must never swallow a control's mouse click.
Basic.ToolTip {
    id: hint
    required property Item anchorItem
    parent: anchorItem
    font: anchorItem.font
    delay: 500
    focus: false
    enabled: false
    closePolicy: Popup.NoAutoClose
    width: Math.min(implicitWidth, 420, Overlay.overlay ? Overlay.overlay.width - 16 : 420)
    x: (anchorItem.width - width) / 2
    y: Overlay.overlay && anchorItem.mapToItem(Overlay.overlay, 0, 0).y + anchorItem.height + implicitHeight + 8 < Overlay.overlay.height ? anchorItem.height + 6 : -implicitHeight - 6
    background: Rectangle {
        radius: 4
        color: Kirigami.Theme.backgroundColor
        border.color: Qt.alpha(Kirigami.Theme.textColor, 0.3)
    }
    contentItem: Text {
        text: hint.text
        font: hint.font
        color: Kirigami.Theme.textColor
        wrapMode: Text.WrapAnywhere
    }
}
