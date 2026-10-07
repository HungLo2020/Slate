import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Slate.Native

Kirigami.ApplicationWindow {
    id: root
    objectName: "slateWindow"
    width: 1360
    height: 820
    minimumWidth: 480
    minimumHeight: 320
    visible: true
    title: "Slate"
    property var frame: slate.frame
    // Fetch action availability and configured shortcuts only when a menu opens.
    property var menuCommands: []
    property int priorFocus: -1
    property string priorPaneKey: ""
    // Pane delegates are recreated when compact layouts change their visible
    // pane IDs. Keep the repository draft and pending operation above them.
    property string gitCommitDraft: ""
    property string gitSubmittedMessage: ""
    property bool gitCommitting: false
    // Tool views receive their own metadata, not editor/PTY cell snapshots.
    readonly property var gitFrame: ({
            "git_repository": frame.git_repository,
            "git_branch": frame.git_branch,
            "git_busy": frame.git_busy,
            "git_error": frame.git_error,
            "git": frame.git || [],
            "focus": frame.focus
        })
    // One measured header size is shared with Rust's editor/PTY viewport calculation.
    FontMetrics {
        id: uiMetrics
        font: root.font
    }
    readonly property int paneHeaderHeight: Math.ceil(Math.max(40, uiMetrics.height + 20))
    readonly property int uiTabMinimum: Math.ceil(Math.max(122, uiMetrics.averageCharacterWidth * 10 + 52))
    readonly property int fileRowHeight: Math.ceil(Math.max(28, uiMetrics.height + 12))
    function syncViewport() {
        slate.paneHeader(paneHeaderHeight + 3);
        slate.viewport(workspace.width, workspace.height);
    }
    onPaneHeaderHeightChanged: Qt.callLater(syncViewport)
    function send(action) {
        slate.send(action);
        slate.refresh();
        if (!frame.prompt && !slate.pathDialogOpen && !slate.closeDialogOpen && !palette.visible && !quitDialog.visible && !settingsDialog.visible && !confirmDiscard.visible)
            Qt.callLater(root.focusPane);
    }
    function pane(id) {
        var items = frame.panes || [];
        for (var i = 0; i < items.length; i++)
            if (items[i].id === id)
                return items[i];
        return ({
                "id": id,
                "kind": "files",
                "rect": {
                    "x": 0,
                    "y": 0,
                    "width": 0,
                    "height": 0
                },
                "tabs": []
            });
    }
    function handle(id) {
        var items = frame.handles || [];
        for (var i = 0; i < items.length; i++)
            if (items[i].id === id)
                return items[i];
        return ({
                "id": id,
                "axis": "horizontal",
                "rect": {
                    "x": 0,
                    "y": 0,
                    "width": 0,
                    "height": 0
                },
                "parent": {
                    "x": 0,
                    "y": 0,
                    "width": 1,
                    "height": 1
                }
            });
    }
    function focusPane() {
        if (slate.pathDialogOpen || slate.closeDialogOpen || palette.visible || editPrompt.visible || settingsDialog.visible || confirmDiscard.visible || quitDialog.visible)
            return;
        for (var i = 0; i < panes.count; i++) {
            var item = panes.itemAt(i);
            if (item && item.paneId === frame.focus)
                item.focusContent();
        }
    }
    function invokeAction(id) {
        if (id === "quit") {
            root.close();
            return;
        }
        if (id === "settings") {
            settingsDialog.open();
            return;
        }
        if (id === "discard-quit" || id === "discard-document") {
            confirmDiscard.actionId = id;
            confirmDiscard.open();
            return;
        }
        root.send({
            "action": "invoke_action",
            "id": id
        });
    }
    function menuCommand(id) {
        return menuCommands.find(function (command) { return command.id === id; }) || {};
    }
    component CommandMenuItem: Basic.MenuItem {
        id: commandItem
        required property string actionId
        property string label: ""
        readonly property var commandInfo: root.menuCommand(actionId)
        objectName: "menuAction_" + actionId
        text: label || commandInfo.name || actionId
        enabled: commandInfo.enabled === true
        font: root.font
        contentItem: RowLayout {
            spacing: 24
            Text {
                text: commandItem.text
                font: commandItem.font
                color: commandItem.enabled ? Kirigami.Theme.textColor : Kirigami.Theme.disabledTextColor
                Layout.fillWidth: true
            }
            Text {
                text: commandItem.commandInfo.shortcut || ""
                font: commandItem.font
                color: Kirigami.Theme.disabledTextColor
            }
        }
        onTriggered: root.invokeAction(actionId)
    }
    function paletteWith(text) {
        commandText.text = text || "";
        palette.open();
        commandText.forceActiveFocus();
        commandText.cursorPosition = commandText.text.length;
    }
    onClosing: function (close) {
        close.accepted = false;
        slate.exit();
        if (slate.frame.dirty)
            quitDialog.open();
    }
    Connections {
        target: slate
        function onPathDialogOpenChanged() {
            if (!slate.pathDialogOpen)
                Qt.callLater(root.focusPane);
        }
        function onCloseDialogOpenChanged() {
            if (!slate.closeDialogOpen)
                Qt.callLater(root.focusPane);
        }
        function onFrameChanged() {
            if (root.gitCommitting)
                Qt.callLater(function () {
                    if (root.gitCommitting && !root.frame.git_busy) {
                        if (!root.frame.git_error && root.gitCommitDraft === root.gitSubmittedMessage)
                            root.gitCommitDraft = "";
                        root.gitCommitting = false;
                    }
                });
            if (root.frame.prompt && root.frame.prompt.kind === "settings") {
                Qt.callLater(function () {
                    settingsDialog.open();
                    root.send({"action": "dismiss_prompt"});
                });
                return;
            }
            if (root.frame.prompt && root.frame.prompt.kind === "close-tab") {
                Qt.callLater(function () {
                    if (root.frame.prompt && root.frame.prompt.kind === "close-tab")
                        slate.confirmCloseTab(root);
                });
                return;
            }
            if (root.frame.prompt && ["open", "open-folder", "save-as"].indexOf(root.frame.prompt.kind) !== -1) {
                Qt.callLater(function () {
                    if (root.frame.prompt && ["open", "open-folder", "save-as"].indexOf(root.frame.prompt.kind) !== -1)
                        slate.pickPath(root.frame.prompt.kind, root);
                });
                return;
            }
            if (root.frame.prompt && !editPrompt.visible) {
                editPrompt.open();
                promptInput.forceActiveFocus();
            } else if (!root.frame.prompt && editPrompt.visible) {
                editPrompt.close();
                Qt.callLater(root.focusPane);
            }
            var paneKey = (root.frame.panes || []).map(function (pane) { return pane.id; }).join(",");
            if ((root.priorFocus !== root.frame.focus || root.priorPaneKey !== paneKey) && !palette.visible && !settingsDialog.visible && !confirmDiscard.visible && !editPrompt.visible) {
                root.priorFocus = root.frame.focus;
                root.priorPaneKey = paneKey;
                Qt.callLater(root.focusPane);
            }
        }
    }
    Shortcut {
        sequence: "F1"
        onActivated: root.paletteWith("")
    }
    Shortcut {
        sequence: "Ctrl+Shift+P"
        onActivated: root.paletteWith("")
    }
    menuBar: Basic.MenuBar {
        id: mainMenuBar
        objectName: "mainMenuBar"
        font: root.font
        delegate: Basic.MenuBarItem {
            objectName: "menu_" + text.replace("&", "")
            font: root.font
            leftPadding: 8
            rightPadding: 8
        }
        background: Rectangle {
            color: Kirigami.Theme.backgroundColor
        }
        Menu {
            objectName: "fileMenu"
            title: "&File"
            onAboutToShow: root.menuCommands = slate.commands("")
            CommandMenuItem { actionId: "new"; label: "New File" }
            CommandMenuItem { actionId: "open"; label: "Open File…" }
            CommandMenuItem { actionId: "open-folder"; label: "Open Folder…" }
            MenuSeparator {}
            CommandMenuItem { actionId: "save"; label: "Save" }
            CommandMenuItem { actionId: "save-as"; label: "Save As…" }
            CommandMenuItem { actionId: "close"; label: "Close Tab" }
            MenuSeparator {}
            CommandMenuItem { actionId: "settings"; label: "Settings…" }
            CommandMenuItem { actionId: "settings-reload"; label: "Reload Settings" }
            MenuSeparator {}
            CommandMenuItem { actionId: "quit"; label: "Quit" }
        }
        Menu {
            objectName: "editMenu"
            title: "&Edit"
            onAboutToShow: root.menuCommands = slate.commands("")
            CommandMenuItem { actionId: "undo" }
            CommandMenuItem { actionId: "redo" }
            MenuSeparator {}
            CommandMenuItem { actionId: "cut"; label: "Cut" }
            CommandMenuItem { actionId: "copy"; label: "Copy" }
            CommandMenuItem { actionId: "paste" }
            CommandMenuItem { actionId: "select-all" }
            MenuSeparator {}
            CommandMenuItem { actionId: "prompt-find" }
            CommandMenuItem { actionId: "prompt-replace" }
            MenuSeparator {}
            CommandMenuItem { actionId: "indent" }
            CommandMenuItem { actionId: "outdent" }
        }
        Menu {
            objectName: "viewMenu"
            title: "&View"
            onAboutToShow: root.menuCommands = slate.commands("")
            MenuItem {
                objectName: "menuAction_commands"
                text: "Command Palette…"
                onTriggered: root.paletteWith("")
            }
            MenuSeparator {}
            CommandMenuItem {
                actionId: "toggle-workspace"
                label: root.frame.editor_only ? "Expand Workspace" : "Collapse to Editor"
            }
            CommandMenuItem { actionId: "files"; label: "File Browser" }
            CommandMenuItem { actionId: "git"; label: "Git Changes" }
            MenuSeparator {}
            Menu {
                title: "Layout"
                CommandMenuItem { actionId: "preset development"; label: "Three Panes" }
                CommandMenuItem { actionId: "preset bottom_terminal"; label: "Terminal Below" }
                CommandMenuItem { actionId: "editor-only"; label: "Editor Only" }
                MenuSeparator {}
                CommandMenuItem { actionId: "layout-save"; label: "Save Layout…" }
                CommandMenuItem { actionId: "layout-load"; label: "Load Layout…" }
            }
            CommandMenuItem { actionId: "split-right" }
            CommandMenuItem { actionId: "split-down" }
            CommandMenuItem { actionId: "close-pane" }
        }
        Menu {
            objectName: "goMenu"
            title: "&Go"
            onAboutToShow: root.menuCommands = slate.commands("")
            CommandMenuItem { actionId: "prompt-goto" }
            CommandMenuItem { actionId: "find-next" }
            CommandMenuItem { actionId: "find-previous" }
            MenuSeparator {}
            CommandMenuItem { actionId: "next-tab" }
            CommandMenuItem { actionId: "next-pane" }
        }
        Menu {
            objectName: "terminalMenu"
            title: "&Terminal"
            onAboutToShow: root.menuCommands = slate.commands("")
            CommandMenuItem { actionId: "terminal" }
            CommandMenuItem { actionId: "terminate-terminal"; label: "Close Terminal" }
            MenuSeparator {}
            CommandMenuItem { actionId: "split-terminal-right" }
            CommandMenuItem { actionId: "split-terminal-down" }
        }
    }
    Item {
        id: workspace
        anchors.fill: parent
        onWidthChanged: root.syncViewport()
        onHeightChanged: root.syncViewport()
        Component.onCompleted: {
            root.syncViewport();
            Qt.callLater(root.focusPane);
        }
        Repeater {
            id: panes
            model: slate.paneIds
            delegate: Rectangle {
                id: panel
                required property var modelData
                property int paneId: modelData
                property var paneData: root.pane(paneId)
                property var tabItems: []
                property string tabKey: ""
                function updateTabs() {
                    var next = paneData && paneData.tabs ? paneData.tabs : [];
                    var key = JSON.stringify(next);
                    // Document/PTY frames change pane metadata frequently.
                    // Rebuilding buttons, hints and menu items on every frame
                    // overwhelms QML's garbage collector during editing.
                    if (key !== tabKey) {
                        tabKey = key;
                        tabItems = next;
                    }
                }
                onPaneDataChanged: updateTabs()
                Component.onCompleted: updateTabs()
                x: paneData.rect.x
                y: paneData.rect.y
                width: paneData.rect.width
                height: paneData.rect.height
                color: Kirigami.Theme.backgroundColor
                border.color: root.frame.focus === paneId ? Kirigami.Theme.highlightColor : Kirigami.Theme.disabledTextColor
                border.width: root.frame.focus === paneId ? 2 : 1
                clip: true
                function focusContent() {
                    if (paneData.kind === "editor" || paneData.kind === "terminal")
                        grid.forceActiveFocus();
                    else if (paneData.kind === "git")
                        gitView.focusContent();
                    else
                        browser.forceActiveFocus();
                }
                RowLayout {
                    id: paneHeader
                    objectName: "paneHeader_" + panel.paneId
                    x: 2
                    y: 2
                    width: Math.max(0, parent.width - 4)
                    height: root.paneHeaderHeight
                    spacing: Kirigami.Units.smallSpacing
                    Item {
                        id: tabs
                        objectName: "tabs_" + panel.paneId
                        Layout.fillWidth: true
                        Layout.minimumWidth: 0
                        Layout.fillHeight: true
                        readonly property var entries: panel.tabItems
                        readonly property bool crowded: entries.length * root.uiTabMinimum > width
                        RowLayout {
                            anchors.fill: parent
                            spacing: 4
                            Repeater {
                                model: tabs.entries
                                delegate: Item {
                                    id: fileTab
                                    required property var modelData
                                    required property int index
                                    objectName: "tab_" + panel.paneId + "_" + index
                                    visible: !tabs.crowded || modelData.active
                                    Layout.fillWidth: true
                                    Layout.minimumWidth: 0
                                    Layout.maximumWidth: tabs.crowded ? Infinity : 240
                                    implicitWidth: tabSelect.implicitWidth
                                    implicitHeight: tabSelect.implicitHeight
                                    ActionButton {
                                        id: tabSelect
                                        objectName: "tabSelect_" + panel.paneId + "_" + fileTab.index
                                        anchors.fill: parent
                                        rightPadding: tabClose.visible ? tabClose.width + 8 : leftPadding
                                        text: fileTab.modelData.title
                                        tip: fileTab.modelData.title
                                        highlighted: fileTab.modelData.active
                                        font.bold: fileTab.modelData.active
                                        onClicked: root.send({
                                            "action": "switch_tab",
                                            "pane": panel.paneId,
                                            "index": fileTab.index
                                        })
                                    }
                                    ActionButton {
                                        id: tabClose
                                        objectName: "tabClose_" + panel.paneId + "_" + fileTab.modelData.close_id
                                        visible: fileTab.modelData.close_id !== null && fileTab.modelData.close_id !== undefined
                                        anchors.right: parent.right
                                        anchors.rightMargin: 4
                                        anchors.verticalCenter: parent.verticalCenter
                                        compact: true
                                        text: "×"
                                        horizontalPadding: 5
                                        width: Math.max(implicitWidth, implicitHeight)
                                        height: implicitHeight
                                        flat: true
                                        foregroundColor: fileTab.modelData.active ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                                        background: Rectangle {
                                            radius: 4
                                            color: tabClose.down ? Qt.alpha(tabClose.foregroundColor, 0.24) : tabClose.hovered ? Qt.alpha(tabClose.foregroundColor, 0.12) : "transparent"
                                            border.width: tabClose.visualFocus ? 2 : 0
                                            border.color: tabClose.foregroundColor
                                        }
                                        tip: (fileTab.modelData.editor_id === null ? "Stop and close " : "Close ") + fileTab.modelData.title
                                        Accessible.name: tip
                                        onClicked: root.send({
                                            "action": "close_tab",
                                            "pane": panel.paneId,
                                            "view": fileTab.modelData.close_id
                                        })
                                    }
                                }
                            }
                            Item {
                                Layout.fillWidth: true
                                visible: !tabs.crowded
                            }
                            ActionButton {
                                objectName: "tabOverflow_" + panel.paneId
                                visible: tabs.crowded && tabs.entries.length > 1
                                text: "▾"
                                horizontalPadding: 6
                                Layout.minimumWidth: implicitHeight
                                Layout.preferredWidth: implicitHeight
                                tip: "All tabs (" + tabs.entries.length + ")"
                                Accessible.name: tip
                                flat: true
                                onClicked: tabMenu.popup()
                                Menu {
                                    id: tabMenu
                                    Instantiator {
                                        model: tabs.entries
                                        delegate: MenuItem {
                                            required property var modelData
                                            required property int index
                                            text: modelData.title
                                            checkable: true
                                            checked: modelData.active
                                            onTriggered: root.send({
                                                "action": "switch_tab",
                                                "pane": panel.paneId,
                                                "index": index
                                            })
                                        }
                                        onObjectAdded: function (index, object) {
                                            tabMenu.insertItem(index, object);
                                        }
                                        onObjectRemoved: function (index, object) {
                                            tabMenu.removeItem(object);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    ActionButton {
                        objectName: "paneActions_" + panel.paneId
                        iconName: "view-more-symbolic"
                        tip: "Pane actions"
                        flat: true
                        Accessible.name: "Pane actions"
                        onClicked: {
                            root.send({
                                "action": "focus",
                                "pane": panel.paneId
                            });
                            paneMenu.popup();
                        }
                    }
                }
                CellView {
                    id: grid
                    objectName: "cells_" + panel.paneId
                    x: 1
                    y: root.paneHeaderHeight + 2
                    width: Math.max(0, parent.width - 2)
                    height: Math.max(0, parent.height - root.paneHeaderHeight - 3)
                    visible: panel.paneData.kind === "editor" || panel.paneData.kind === "terminal"
                    paneId: panel.paneId
                    Accessible.role: panel.paneData.kind === "editor" ? Accessible.EditableText : Accessible.Pane
                    Accessible.name: (panel.paneData.tabs || []).filter(function (t) {
                        return t.active;
                    }).map(function (t) {
                        return t.title;
                    }).join("")
                    Accessible.description: panel.paneData.read_only ? "Read-only inspection. " + (panel.paneData.editor ? panel.paneData.editor.surrounding : "") : "Text editor. " + (panel.paneData.editor ? panel.paneData.editor.surrounding : "")
                }
                ScrollBar {
                    id: editorScroll
                    objectName: "editorScroll_" + panel.paneId
                    visible: panel.paneData.kind === "editor" && !!panel.paneData.editor
                    x: Math.max(0, panel.width - width - 2)
                    y: grid.y
                    height: grid.height
                    orientation: Qt.Vertical
                    policy: ScrollBar.AsNeeded
                    size: panel.paneData.editor ? Math.min(1, panel.paneData.rows / Math.max(1, panel.paneData.editor.line_count)) : 1
                    position: panel.paneData.editor ? panel.paneData.editor.top / Math.max(1, panel.paneData.editor.line_count) : 0
                    onPositionChanged: if (pressed)
                        root.send({
                            "action": "scroll_to",
                            "pane": panel.paneId,
                            "line": Math.round(position * panel.paneData.editor.line_count)
                        })
                    Accessible.name: "Document scroll position"
                }
                GitPane {
                    id: gitView
                    objectName: "gitView_" + panel.paneId
                    x: 2
                    y: root.paneHeaderHeight + 3
                    width: Math.max(0, panel.width - 4)
                    height: Math.max(0, panel.height - y - 2)
                    visible: panel.paneData.kind === "git"
                    bridge: slate
                    frame: root.gitFrame
                    paneData: ({
                            "selected": panel.paneData.selected || 0
                        })
                    paneId: panel.paneId
                    draft: root.gitCommitDraft
                    committing: root.gitCommitting
                    onDraftEdited: function (text) {
                        root.gitCommitDraft = text;
                    }
                    onCommitRequested: function (message) {
                        root.gitSubmittedMessage = message;
                        root.gitCommitting = true;
                        slate.send({
                            "action": "git_commit",
                            "message": message
                        });
                        slate.refresh();
                    }
                    onActionRequested: function (action) {
                        slate.send(action);
                        slate.refresh();
                    }
                }
                ListView {
                    id: browser
                    objectName: (visible ? "browser_" : "hiddenBrowser_") + panel.paneId
                    x: 1
                    y: root.paneHeaderHeight + 2
                    width: Math.max(0, parent.width - 6 - Math.max(12, fileScroll.implicitWidth))
                    height: Math.max(0, parent.height - y - 1)
                    visible: panel.paneData.kind === "files"
                    clip: true
                    model: slate.files
                    currentIndex: panel.paneData.selected || 0
                    boundsBehavior: Flickable.StopAtBounds
                    ScrollBar.vertical: Basic.ScrollBar {
                        id: fileScroll
                        objectName: "fileScroll_" + panel.paneId
                        parent: panel
                        x: browser.width + 4
                        y: browser.y
                        width: Math.max(12, implicitWidth)
                        height: browser.height
                        visible: browser.visible && size < 1
                    }
                    Keys.onPressed: function (event) {
                        slate.key(event.key, event.text, event.modifiers);
                        event.accepted = true;
                    }
                    Label {
                        textFormat: Text.PlainText
                        anchors.centerIn: parent
                        width: Math.max(0, parent.width - 20)
                        visible: browser.count === 0
                        text: "No files to show"
                        wrapMode: Text.Wrap
                        horizontalAlignment: Text.AlignHCenter
                    }
                    delegate: Basic.ItemDelegate {
                        id: fileDelegate
                        objectName: "entry_" + panel.paneId + "_" + index
                        required property var modelData
                        required property int index
                        height: root.fileRowHeight
                        width: browser.width
                        padding: 4
                        contentItem: Label {
                            textFormat: Text.PlainText
                            text: fileDelegate.text
                            font: fileDelegate.font
                            elide: Text.ElideMiddle
                            verticalAlignment: Text.AlignVCenter
                            color: fileDelegate.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                        }
                        background: Rectangle {
                            color: fileDelegate.highlighted ? Kirigami.Theme.highlightColor : fileDelegate.hovered ? Kirigami.Theme.alternateBackgroundColor : "transparent"
                        }
                        Hint {
                            anchorItem: fileDelegate
                            visible: fileDelegate.hovered
                            text: fileDelegate.modelData.path
                        }
                        text: (modelData.directory ? "▸  " : "   ") + modelData.name
                        highlighted: index === browser.currentIndex
                        onClicked: {
                            root.send({
                                "action": "click",
                                "pane": panel.paneId,
                                "row": index,
                                "col": 0
                            });
                            browser.forceActiveFocus();
                        }
                        onDoubleClicked: {
                            root.send({
                                "action": "focus",
                                "pane": panel.paneId
                            });
                            root.send({
                                "action": "open",
                                "path": modelData.path
                            });
                        }
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.RightButton
                    enabled: panel.paneData.kind !== "terminal" && panel.paneData.kind !== "git"
                    onClicked: function (mouse) {
                        if (browser.visible) {
                            var index = browser.indexAt(mouse.x - browser.x, mouse.y - browser.y + browser.contentY);
                            if (index >= 0)
                                root.send({
                                    "action": "click",
                                    "pane": panel.paneId,
                                    "row": index,
                                    "col": 0
                                });
                        }
                        root.send({
                            "action": "focus",
                            "pane": panel.paneId
                        });
                        paneMenu.popup();
                    }
                }
                Menu {
                    id: paneMenu
                    x: Math.max(0, panel.width - width - 2)
                    y: root.paneHeaderHeight + 2
                    MenuItem {
                        text: "Save document"
                        enabled: !panel.paneData.read_only
                        visible: panel.paneData.kind === "editor"
                        onTriggered: root.send({
                            "action": "save"
                        })
                    }
                    MenuItem {
                        text: "Find…"
                        visible: panel.paneData.kind === "editor"
                        onTriggered: root.send({
                            "action": "prompt",
                            "kind": "find"
                        })
                    }
                    MenuItem {
                        text: "New terminal"
                        visible: panel.paneData.kind === "terminal"
                        onTriggered: root.send({
                            "action": "new_terminal"
                        })
                    }
                    MenuItem {
                        text: "Split right"
                        onTriggered: root.send({
                            "action": "split",
                            "axis": "horizontal"
                        })
                    }
                    MenuItem {
                        text: "Split below"
                        onTriggered: root.send({
                            "action": "split",
                            "axis": "vertical"
                        })
                    }
                    MenuSeparator {}
                    MenuItem {
                        text: "Copy selection"
                        onTriggered: slate.copyClipboard()
                    }
                    MenuItem {
                        text: "Paste"
                        onTriggered: slate.pasteClipboard()
                    }
                    MenuSeparator {}
                    MenuItem {
                        text: "Files view"
                        onTriggered: root.send({
                            "action": "add_view",
                            "kind": "files"
                        })
                    }
                    MenuItem {
                        text: "Git view"
                        onTriggered: root.send({
                            "action": "add_view",
                            "kind": "git"
                        })
                    }
                    MenuItem {
                        text: "Editor view"
                        onTriggered: root.send({
                            "action": "add_view",
                            "kind": "editor"
                        })
                    }
                    MenuItem {
                        text: "Terminal view"
                        onTriggered: root.send({
                            "action": "new_terminal"
                        })
                    }
                    MenuSeparator {}
                    MenuItem {
                        text: "Close pane"
                        onTriggered: root.send({
                            "action": "close_pane"
                        })
                    }
                    MenuItem {
                        text: "Move/swap pane…"
                        onTriggered: root.invokeAction("move-pane")
                    }
                    MenuItem {
                        text: "Terminate terminal"
                        visible: panel.paneData.kind === "terminal"
                        onTriggered: root.send({
                            "action": "terminate_terminal"
                        })
                    }
                    MenuSeparator {}
                    MenuItem {
                        text: "Stage selected"
                        enabled: (root.frame.commands || []).some(function (c) {
                            return c.id === "stage" && c.enabled;
                        })
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.invokeAction("stage")
                    }
                    MenuItem {
                        text: "Unstage selected"
                        enabled: (root.frame.commands || []).some(function (c) {
                            return c.id === "unstage" && c.enabled;
                        })
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.invokeAction("unstage")
                    }
                    MenuItem {
                        text: "View diff"
                        enabled: (root.frame.commands || []).some(function (c) {
                            return c.id === "diff" && c.enabled;
                        })
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.invokeAction("diff")
                    }
                    MenuItem {
                        text: "Commit…"
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.invokeAction("commit")
                    }
                }
            }
        }
        Repeater {
            model: slate.handleIds
            delegate: Rectangle {
                required property var modelData
                property var handleData: root.handle(modelData)
                x: handleData.rect.x
                y: handleData.rect.y
                width: handleData.rect.width
                height: handleData.rect.height
                color: drag.containsMouse ? Kirigami.Theme.highlightColor : Kirigami.Theme.disabledTextColor
                MouseArea {
                    id: drag
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: parent.handleData.axis === "horizontal" ? Qt.SplitHCursor : Qt.SplitVCursor
                    onPositionChanged: function (mouse) {
                        if (pressed) {
                            var p = mapToItem(workspace, mouse.x, mouse.y);
                            var h = parent.handleData;
                            var ratio = h.axis === "horizontal" ? (p.x - h.parent.x) / h.parent.width : (p.y - h.parent.y) / h.parent.height;
                            root.send({
                                "action": "resize_split",
                                "id": h.id,
                                "ratio": ratio
                            });
                        }
                    }
                }
            }
        }
    }
    footer: Basic.ToolBar {
        background: Rectangle {
            color: Kirigami.Theme.backgroundColor
        }
        implicitHeight: statusRow.implicitHeight + 2 * Kirigami.Units.smallSpacing
        RowLayout {
            id: statusRow
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            Label {
                id: statusLabel
                textFormat: Text.PlainText
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                text: root.frame.hints || ""
                elide: Text.ElideRight
                Hint {
                    anchorItem: statusLabel
                    visible: statusHover.hovered
                    text: statusLabel.text
                }
                HoverHandler {
                    id: statusHover
                }
            }
            Label {
                textFormat: Text.PlainText
                text: root.frame.location || ""
                visible: text.length > 0
                Layout.maximumWidth: root.width * 0.4
                elide: Text.ElideMiddle
            }
            ActionButton {
                text: "?"
                onClicked: root.paletteWith("")
                tip: root.frame.hints || "Keyboard shortcuts"
            }
        }
    }
    Dialog {
        id: editPrompt
        enter: Transition {}
        exit: Transition {}
        objectName: "editPrompt"
        property var prompt: root.frame.prompt || ({
                "kind": "find",
                "input": "",
                "replacement": "",
                "case_sensitive": false,
                "whole_word": false
            })
        onOpened: promptInput.forceActiveFocus()
        title: ({
                "goto": "Go to line",
                "replace": "Replace",
                "find": "Find",
                "open": "Open file or folder",
                "save-as": "Save document as",
                "commit": "Commit staged changes",
                "layout-save": "Save layout",
                "layout-load": "Load layout",
                "move-pane": "Move or swap pane",
                "settings": "Settings"
            })[prompt.kind] || "Input"
        anchors.centerIn: parent
        width: Math.min(root.width - 40, 560)
        height: Math.min(root.height - 40, implicitHeight)
        modal: true
        closePolicy: Popup.CloseOnEscape
        onRejected: root.send({
            "action": "dismiss_prompt"
        })
        function update() {
            root.send({
                "action": "update_prompt",
                "input": promptInput.text,
                "replacement": replacementInput.text,
                "case_sensitive": matchCase.checked,
                "whole_word": wholeWord.checked
            });
        }
        contentItem: ScrollView {
            id: promptScroll
            clip: true
            contentWidth: availableWidth
            ColumnLayout {
                width: promptScroll.availableWidth
                spacing: Kirigami.Units.smallSpacing
                TextField {
                    id: promptInput
                    objectName: "searchInput"
                    Layout.fillWidth: true
                    text: editPrompt.prompt.input
                    placeholderText: ({
                            "goto": "Line number",
                            "open": "File or directory path",
                            "save-as": "New file path",
                            "commit": "Commit message",
                            "layout-save": "Layout name",
                            "layout-load": "Saved layout name",
                            "move-pane": "Target pane number",
                            "settings": "Option and value, e.g. indent-width 4"
                        })[editPrompt.prompt.kind] || "Find text"
                    onTextEdited: editPrompt.update()
                    onAccepted: root.send({
                        "action": "submit_prompt"
                    })
                }
                TextField {
                    id: replacementInput
                    objectName: "replacementInput"
                    Layout.fillWidth: true
                    visible: editPrompt.prompt.kind === "replace"
                    text: editPrompt.prompt.replacement
                    placeholderText: "Replacement text"
                    onTextEdited: editPrompt.update()
                    onAccepted: root.send({
                        "action": "submit_prompt"
                    })
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    visible: editPrompt.prompt.kind === "find" || editPrompt.prompt.kind === "replace"
                    CheckBox {
                        id: matchCase
                        text: "Match case"
                        checked: editPrompt.prompt.case_sensitive
                        onClicked: editPrompt.update()
                    }
                    CheckBox {
                        id: wholeWord
                        text: "Whole word"
                        checked: editPrompt.prompt.whole_word
                        onClicked: editPrompt.update()
                    }
                }
                Label {
                    textFormat: Text.PlainText
                    Layout.fillWidth: true
                    text: root.frame.status || ""
                    wrapMode: Text.Wrap
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    ActionButton {
                        text: editPrompt.prompt.kind === "goto" ? "Go" : editPrompt.prompt.kind === "replace" ? "Replace next" : editPrompt.prompt.kind === "find" ? "Find next" : "Confirm"
                        onClicked: root.send({
                            "action": "submit_prompt"
                        })
                    }
                    ActionButton {
                        text: "Replace all"
                        visible: editPrompt.prompt.kind === "replace"
                        onClicked: root.send({
                            "action": "submit_prompt",
                            "all": true
                        })
                    }
                    ActionButton {
                        text: "Close"
                        onClicked: root.send({
                            "action": "dismiss_prompt"
                        })
                    }
                }
            }
        }
    }
    Dialog {
        id: palette
        enter: Transition {}
        exit: Transition {}
        objectName: "commandPalette"
        title: "Commands"
        anchors.centerIn: parent
        width: Math.min(root.width - 40, 720)
        height: Math.min(root.height - 40, 480)
        modal: true
        standardButtons: Dialog.Cancel
        onOpened: commandText.forceActiveFocus()
        property var results: {
            if (!visible)
                return [];
            var context = root.frame.command_revision;
            return slate.commands(commandText.text);
        }
        function execute() {
            var raw = commandText.text.trim();
            if (raw.charAt(0) === ":") {
                palette.close();
                slate.command(raw.substring(1));
                Qt.callLater(root.focusPane);
                return;
            }
            if (commandResults.currentIndex < 0 || commandResults.currentIndex >= results.length)
                return;
            var action = results[commandResults.currentIndex];
            if (!action.enabled)
                return;
            palette.close();
            root.invokeAction(action.id);
        }
        onRejected: Qt.callLater(root.focusPane)
        contentItem: ColumnLayout {
            TextField {
                id: commandText
                objectName: "commandSearch"
                Layout.fillWidth: true
                placeholderText: "Search actions…"
                onTextChanged: commandResults.currentIndex = 0
                onAccepted: palette.execute()
                Keys.onDownPressed: commandResults.currentIndex = Math.min(commandResults.count - 1, commandResults.currentIndex + 1)
                Keys.onUpPressed: commandResults.currentIndex = Math.max(0, commandResults.currentIndex - 1)
            }
            ListView {
                id: commandResults
                objectName: "commandResults"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: palette.results
                currentIndex: 0
                onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)
                ScrollBar.vertical: ScrollBar {}
                delegate: Basic.ItemDelegate {
                    id: commandResult
                    required property var modelData
                    required property int index
                    width: commandResults.width
                    height: Math.max(60, uiMetrics.height * 2 + 16)
                    opacity: modelData.enabled ? 1 : 0.55
                    highlighted: index === commandResults.currentIndex
                    contentItem: ColumnLayout {
                        RowLayout {
                            Label {
                                textFormat: Text.PlainText
                                text: commandResult.modelData.name
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                elide: Text.ElideRight
                                font.bold: true
                                color: commandResult.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                            }
                            Label {
                                textFormat: Text.PlainText
                                text: commandResult.modelData.shortcut
                                color: commandResult.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.disabledTextColor
                            }
                        }
                        Label {
                            textFormat: Text.PlainText
                            text: commandResult.modelData.enabled ? commandResult.modelData.description : commandResult.modelData.reason
                            Layout.fillWidth: true
                            elide: Text.ElideRight
                            color: commandResult.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.disabledTextColor
                        }
                    }
                    background: Rectangle {
                        color: commandResult.highlighted ? Kirigami.Theme.highlightColor : "transparent"
                        radius: 4
                    }
                    onClicked: {
                        commandResults.currentIndex = index;
                        palette.execute();
                    }
                }
                Label {
                    textFormat: Text.PlainText
                    anchors.centerIn: parent
                    visible: commandResults.count === 0
                    text: "No matching actions"
                }
            }
            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                text: "↑ ↓ select · Enter runs · Escape closes"
                color: Kirigami.Theme.disabledTextColor
            }
        }
    }
    Dialog {
        id: settingsDialog
        enter: Transition {}
        exit: Transition {}
        objectName: "settingsDialog"
        title: "Settings"
        anchors.centerIn: parent
        width: Math.min(root.width - 40, 480)
        height: Math.min(root.height - 40, implicitHeight)
        modal: true
        standardButtons: Dialog.Close
        contentItem: ScrollView {
            id: settingsScroll
            clip: true
            contentWidth: availableWidth
            leftPadding: Kirigami.Units.largeSpacing
            rightPadding: Kirigami.Units.largeSpacing
            topPadding: Kirigami.Units.smallSpacing
            bottomPadding: Kirigami.Units.smallSpacing
            ColumnLayout {
                width: settingsScroll.availableWidth
                Label {
                    text: "Startup"
                    font.bold: true
                }
                Repeater {
                    model: [
                        {"label": "Opening a file", "setting": "file-startup", "field": "file_startup"},
                        {"label": "Opening a directory", "setting": "directory-startup", "field": "directory_startup"}
                    ]
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        Label {
                            text: modelData.label
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            wrapMode: Text.Wrap
                        }
                        ComboBox {
                            objectName: modelData.field === "file_startup" ? "settingsFileStartup" : "settingsDirectoryStartup"
                            model: ["Editor only", "Full workspace"]
                            currentIndex: root.frame.settings && root.frame.settings[modelData.field] === "editor-only" ? 0 : 1
                            onActivated: root.send({"action": "configure", "name": modelData.setting,
                                "value": currentIndex === 0 ? "editor-only" : "workspace"})
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    text: "Applies the next time Slate starts. F10 expands or collapses the current workspace."
                    wrapMode: Text.Wrap
                }
                Label {
                    text: "Editor"
                    font.bold: true
                }
                RowLayout {
                    Label {
                        textFormat: Text.PlainText
                        text: "Indent width"
                        Layout.fillWidth: true
                    }
                    SpinBox {
                        objectName: "settingsIndent"
                        from: 1
                        to: 16
                        value: root.frame.settings ? root.frame.settings.indent_width : 4
                        onValueModified: root.send({
                            "action": "configure",
                            "name": "indent-width",
                            "value": value.toString()
                        })
                    }
                }
                CheckBox {
                    text: "Insert spaces instead of tabs"
                    checked: root.frame.settings ? root.frame.settings.insert_spaces : true
                    onClicked: root.send({
                        "action": "configure",
                        "name": "insert-spaces",
                        "value": checked.toString()
                    })
                }
                CheckBox {
                    text: "Indent new lines automatically"
                    checked: root.frame.settings ? root.frame.settings.auto_indent : true
                    onClicked: root.send({
                        "action": "configure",
                        "name": "auto-indent",
                        "value": checked.toString()
                    })
                }
                CheckBox {
                    text: "Show line numbers"
                    checked: root.frame.settings ? root.frame.settings.line_numbers : true
                    onClicked: root.send({
                        "action": "configure",
                        "name": "line-numbers",
                        "value": checked.toString()
                    })
                }
                RowLayout {
                    Label {
                        textFormat: Text.PlainText
                        text: "Theme"
                        Layout.fillWidth: true
                    }
                    ComboBox {
                        model: ["auto", "dark", "light"]
                        currentIndex: model.indexOf(root.frame.settings ? root.frame.settings.theme : "auto")
                        onActivated: root.send({
                            "action": "configure",
                            "name": "theme",
                            "value": currentText
                        })
                    }
                }
                Label {
                    textFormat: Text.PlainText
                    Layout.fillWidth: true
                    text: "Keyboard bindings can be configured in settings.toml."
                    wrapMode: Text.Wrap
                }
                ActionButton {
                    text: "Open settings file"
                    onClicked: {
                        settingsDialog.close();
                        slate.send({
                            "action": "open_settings"
                        });
                        slate.refresh();
                    }
                }
            }
        }
        onClosed: Qt.callLater(root.focusPane)
    }
    Dialog {
        id: confirmDiscard
        enter: Transition {}
        exit: Transition {}
        property string actionId
        title: "Discard unsaved changes?"
        width: Math.min(root.width - 40, 440)
        anchors.centerIn: parent
        modal: true
        standardButtons: Dialog.Discard | Dialog.Cancel
        Label {
            textFormat: Text.PlainText
            text: confirmDiscard.actionId === "discard-quit" ? "Discard all unsaved documents and quit?" : "Discard this document's unsaved changes?"
            wrapMode: Text.Wrap
            width: confirmDiscard.availableWidth
        }
        onDiscarded: root.send({
            "action": "invoke_action",
            "id": actionId
        })
    }
    Dialog {
        id: quitDialog
        objectName: "quitDialog"
        enter: Transition {}
        exit: Transition {}
        title: "Unsaved documents"
        anchors.centerIn: parent
        modal: true
        standardButtons: Dialog.Discard | Dialog.Cancel
        width: Math.min(root.width - 40, 480)
        Label {
            textFormat: Text.PlainText
            width: quitDialog.availableWidth
            text: "Discard all unsaved changes and quit?"
            wrapMode: Text.Wrap
        }
        onDiscarded: root.send({
            "action": "quit",
            "force": true
        })
    }
}
