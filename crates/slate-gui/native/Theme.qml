pragma Singleton
import QtQuick

// Desktop colours and spacing from the platform palette. Slate needs no
// toolkit beyond Qt Quick Controls, so it runs on any desktop with Qt 6.
QtObject {
    readonly property SystemPalette active: SystemPalette {
        colorGroup: SystemPalette.Active
    }
    readonly property SystemPalette disabled: SystemPalette {
        colorGroup: SystemPalette.Disabled
    }
    readonly property bool dark: (active.window.r * 0.299 + active.window.g * 0.587 + active.window.b * 0.114) < 0.5
    readonly property color textColor: active.windowText
    readonly property color backgroundColor: active.window
    readonly property color viewBackgroundColor: active.base
    readonly property color alternateBackgroundColor: active.alternateBase
    readonly property color highlightColor: active.highlight
    readonly property color highlightedTextColor: active.highlightedText
    readonly property color disabledTextColor: Qt.rgba(active.windowText.r, active.windowText.g, active.windowText.b, 0.55)
    readonly property color negativeTextColor: dark ? "#ed5a6a" : "#c0293b"
    readonly property color neutralTextColor: dark ? "#f6a14b" : "#b65c00"
    readonly property color positiveTextColor: dark ? "#3dd68c" : "#1e7f4c"
    readonly property int smallSpacing: 4
    readonly property int largeSpacing: 8
    // Git change kinds, shared by the file tree and the Git pane: additions
    // green, edits and renames amber, deletions and conflicts red.
    function statusColor(kind) {
        switch (kind) {
        case "added":
        case "untracked":
            return positiveTextColor;
        case "deleted":
        case "conflict":
            return negativeTextColor;
        case "modified":
        case "renamed":
            return neutralTextColor;
        default:
            return textColor;
        }
    }
}
