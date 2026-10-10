import QtQuick
import Slate.Native

// A tool pane's colours (the file browser's or Git pane's), with the same
// names as Theme. `colors` comes from the core's snapshot: the editor's
// palette by default, a fixed one, or — when `desktop` is set — the desktop
// palette Theme already follows. Popups (menus, hints, dialogs) stay on Theme.
QtObject {
    id: pane
    property var colors: null
    readonly property bool desktop: !colors || !!colors.desktop
    readonly property color backgroundColor: desktop ? Theme.backgroundColor : colors.background
    readonly property color textColor: desktop ? Theme.textColor : colors.foreground
    readonly property bool dark: desktop ? Theme.dark : (backgroundColor.r * 0.299 + backgroundColor.g * 0.587 + backgroundColor.b * 0.114) < 0.5
    // Fields sit slightly apart from the pane, as text boxes do on desktops.
    readonly property color viewBackgroundColor: desktop ? Theme.viewBackgroundColor : dark ? Qt.lighter(backgroundColor, 1.35) : Qt.darker(backgroundColor, 1.03)
    readonly property color alternateBackgroundColor: desktop ? Theme.alternateBackgroundColor : Qt.tint(backgroundColor, Qt.alpha(textColor, 0.05))
    readonly property color highlightColor: desktop ? Theme.highlightColor : colors.selection
    readonly property color highlightedTextColor: desktop ? Theme.highlightedTextColor : colors.selection_foreground
    // The selection while the pane does not have focus.
    readonly property color softHighlightColor: Qt.alpha(highlightColor, desktop ? 0.28 : 0.6)
    readonly property color accentColor: desktop ? Theme.highlightColor : colors.accent
    readonly property color disabledTextColor: Qt.alpha(textColor, 0.55)
    readonly property color negativeTextColor: dark ? "#ed5a6a" : "#c0293b"
    readonly property color neutralTextColor: dark ? "#f6a14b" : "#b65c00"
    readonly property color positiveTextColor: dark ? "#3dd68c" : "#1e7f4c"
    readonly property int smallSpacing: Theme.smallSpacing
    readonly property int largeSpacing: Theme.largeSpacing
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
