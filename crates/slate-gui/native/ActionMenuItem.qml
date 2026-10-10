import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import QtQuick.Layouts
import Slate.Native

// A menu entry styled like the window's catalog entries. Captions name files,
// branches and commits, so they are always plain text, never markup.
Basic.MenuItem {
    id: item
    required property string caption
    property string shortcut: ""
    text: caption
    height: visible ? implicitHeight : 0
    contentItem: RowLayout {
        spacing: 24
        Text {
            Layout.fillWidth: true
            text: item.caption
            textFormat: Text.PlainText
            elide: Text.ElideMiddle
            font: item.font
            color: item.highlighted ? Theme.highlightedTextColor : item.enabled ? Theme.textColor : Theme.disabledTextColor
        }
        Text {
            visible: item.shortcut.length > 0
            text: item.shortcut
            textFormat: Text.PlainText
            font: item.font
            color: item.highlighted ? Theme.highlightedTextColor : Theme.disabledTextColor
        }
    }
    background: Rectangle {
        color: item.highlighted ? Theme.highlightColor : "transparent"
    }
}
