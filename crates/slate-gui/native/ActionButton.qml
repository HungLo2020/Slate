import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import org.kde.kirigami as Kirigami

// Own the caption and background together: a native KDE button background can
// paint its own caption, and its style insets can escape a tightly sized row.
Basic.Button {
    id: control
    property string iconName: ""
    property string tip: ""
    property color foregroundColor: Kirigami.Theme.textColor
    readonly property string fallbackGlyph: ({
            "list-add": "+",
            "list-remove": "−",
            "view-refresh": "↻",
            "view-more-symbolic": "⋮",
            "arrow-down": "▾",
            "vcs-diff": "≠"
        })[iconName] || "⋯"
    flat: false
    property bool compact: false
    readonly property bool iconOnly: text.length === 0
    readonly property real iconExtent: Math.ceil(Math.max(16, metrics.height))
    readonly property real minimumExtent: Math.ceil(Math.max(compact ? 28 : 32, metrics.height + (compact ? 10 : 16)))
    FontMetrics {
        id: metrics
        font: control.font
    }
    leftInset: 0
    rightInset: 0
    topInset: 0
    bottomInset: 0
    padding: compact ? 5 : 8
    horizontalPadding: iconOnly ? padding : 10
    implicitHeight: minimumExtent
    implicitWidth: iconOnly ? minimumExtent : Math.ceil(caption.implicitWidth + 2 * horizontalPadding + (iconName.length ? iconExtent + 6 : 0))
    hoverEnabled: true
    Accessible.name: text.length ? text : tip
    Hint {
        anchorItem: control
        visible: control.hovered && control.tip.length > 0
        text: control.tip
    }
    contentItem: Item {
        Kirigami.Icon {
            id: glyph
            visible: control.iconName.length > 0
            source: control.iconName
            width: control.iconExtent
            height: width
            anchors.verticalCenter: parent.verticalCenter
            x: control.iconOnly ? (parent.width - width) / 2 : 0
            color: caption.color
        }
        Text {
            visible: control.iconOnly && !glyph.valid
            anchors.fill: parent
            text: control.fallbackGlyph
            font: control.font
            color: caption.color
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
        Text {
            id: caption
            visible: !control.iconOnly
            x: glyph.visible ? glyph.width + 6 : 0
            width: Math.max(0, parent.width - x)
            height: parent.height
            text: control.text
            font: control.font
            elide: Text.ElideMiddle
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            color: !control.enabled ? Kirigami.Theme.disabledTextColor : control.highlighted || control.down ? Kirigami.Theme.highlightedTextColor : control.foregroundColor
        }
    }
    background: Rectangle {
        radius: 4
        color: control.highlighted || control.down ? Kirigami.Theme.highlightColor : control.hovered ? Qt.tint(Kirigami.Theme.backgroundColor, Qt.alpha(Kirigami.Theme.highlightColor, 0.12)) : control.flat ? "transparent" : Kirigami.Theme.backgroundColor
        border.width: control.visualFocus ? 2 : control.flat && !control.hovered ? 0 : 1
        border.color: control.visualFocus || control.highlighted ? Kirigami.Theme.highlightColor : Qt.alpha(Kirigami.Theme.textColor, control.enabled ? 0.24 : 0.1)
        opacity: control.enabled ? 1 : 0.6
    }
}
