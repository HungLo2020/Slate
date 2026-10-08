import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import QtQuick.Layouts
import Slate.Native

ApplicationWindow {
    id: root
    objectName: "slateWindow"
    width: 1360
    height: 820
    minimumWidth: 480
    minimumHeight: 320
    visible: true
    title: frame.title || "Slate"
    color: Theme.backgroundColor
    property var frame: slate.frame
    property int priorFocus: -1
    property string priorPaneKey: ""
    // Pane delegates are recreated when compact layouts change their visible
    // pane IDs. Keep the repository draft and pending operation above them.
    property string gitCommitDraft: ""
    property string gitSubmittedMessage: ""
    property bool gitCommitting: false
    readonly property var settings: frame.settings || ({})
    readonly property bool statusIsError: {
        var s = frame.status || "";
        return s.indexOf("Error") === 0 || s.indexOf("failed") !== -1 || s.indexOf("Failed") === 0;
    }
    // Tool views receive their own metadata, not editor/PTY cell snapshots.
    // It is replaced only when repository state or focus changes, so Git
    // controls do not re-evaluate (or query the catalog) while typing.
    property var gitFrame: ({ "git": [] })
    property string gitKey: ""
    function updateGitFrame() {
        // Row actions also depend on which panes show Git, so pane kinds count.
        var kinds = (frame.panes || []).map(function (p) { return p.id + ":" + p.kind; }).join(",");
        var key = [frame.git_revision, frame.git_busy, frame.git_error, frame.git_branch, frame.git_repository, frame.git_restricted, frame.focus, frame.editor_only, kinds].join("|");
        if (key === gitKey)
            return;
        gitKey = key;
        gitFrame = {
            "git_repository": frame.git_repository,
            "git_restricted": frame.git_restricted,
            "git_branch": frame.git_branch,
            "git_busy": frame.git_busy,
            "git_error": frame.git_error,
            "git": frame.git || [],
            "focus": frame.focus
        };
    }
    Component.onCompleted: updateGitFrame()
    // One measured header size is shared with Rust's editor/PTY viewport calculation.
    FontMetrics {
        id: uiMetrics
        font: root.font
    }
    readonly property int paneHeaderHeight: Math.ceil(Math.max(40, uiMetrics.height + 20))
    readonly property int uiTabMinimum: Math.ceil(Math.max(122, uiMetrics.averageCharacterWidth * 10 + 52))
    readonly property int fileRowHeight: Math.ceil(Math.max(28, uiMetrics.height + 12))
    readonly property int minimapWidth: 72
    function syncViewport() {
        slate.paneHeader(paneHeaderHeight + 3);
        slate.viewport(workspace.width, workspace.height);
    }
    onPaneHeaderHeightChanged: Qt.callLater(syncViewport)
    function anyDialogOpen() {
        return slate.pathDialogOpen || slate.closeDialogOpen || commandPalette.visible || editPrompt.visible || settingsDialog.visible || confirmDiscard.visible || quitDialog.visible || choiceDialog.visible || pickerDialog.visible;
    }
    function send(action) {
        slate.send(action);
        slate.refresh();
        if (!frame.prompt && !anyDialogOpen())
            Qt.callLater(root.focusPane);
    }
    // High-frequency input (divider drags, scrollbar drags) coalesces refreshes.
    function sendLater(action) {
        slate.send(action);
        slate.scheduleRefresh();
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
        if (anyDialogOpen())
            return;
        for (var i = 0; i < panes.count; i++) {
            var item = panes.itemAt(i);
            if (item && item.paneId === frame.focus)
                item.focusContent();
        }
    }
    function invokeAction(id, argument, paneId, row) {
        if (paneId) {
            root.send({"action": "focus", "pane": paneId});
            if (row !== undefined && row >= 0)
                root.send({"action": "click", "pane": paneId, "row": row, "col": 0});
        }
        root.send({"action": "invoke_action", "id": id, "argument": argument || ""});
    }
    // Git controls depend on repository state only, not on every editor frame.
    function gitCommandInfo(id, paneId, row) {
        var dependency = root.gitKey;
        return slate.commandInfo(id, paneId || 0, row === undefined ? -1 : row);
    }
    // Menus read the shared catalog when they open, never on every frame.
    function refreshMenu(menu) {
        for (var i = 0; i < menu.count; i++) {
            var item = menu.itemAt(i);
            if (item && item.refreshInfo)
                item.refreshInfo();
        }
        // Lay the list out now, so it never moves under the pointer.
        if (menu.contentItem && menu.contentItem.forceLayout)
            menu.contentItem.forceLayout();
    }
    component CommandMenu: Menu {
        id: commandMenu
        onAboutToShow: root.refreshMenu(commandMenu)
    }
    component CommandMenuItem: Basic.MenuItem {
        id: commandItem
        required property string actionId
        property string label: ""
        property string argument: ""
        property int commandPane: 0
        property var commandInfo: ({})
        function refreshInfo() {
            commandInfo = slate.commandInfo(actionId, commandPane, -1);
        }
        // Names and shortcuts are known before the first open; availability
        // is refreshed whenever the menu opens.
        Component.onCompleted: refreshInfo()
        objectName: "menuAction_" + actionId + (argument ? "_" + argument : "")
        text: label || commandInfo.name || actionId
        enabled: commandInfo.enabled === true
        height: visible ? implicitHeight : 0
        font: root.font
        contentItem: RowLayout {
            spacing: 24
            Text {
                text: (commandItem.checkable ? (commandItem.checked ? "✓  " : "    ") : "") + commandItem.text
                font: commandItem.font
                color: commandItem.highlighted ? Theme.highlightedTextColor : commandItem.enabled ? Theme.textColor : Theme.disabledTextColor
                Layout.fillWidth: true
            }
            Text {
                text: commandItem.commandInfo.shortcut || ""
                font: commandItem.font
                color: commandItem.highlighted ? Theme.highlightedTextColor : Theme.disabledTextColor
            }
        }
        background: Rectangle {
            color: commandItem.highlighted ? Theme.highlightColor : "transparent"
        }
        onTriggered: root.invokeAction(actionId, argument, commandPane)
    }
    // A label with the characters at `positions` bold and underlined.
    function marked(label, positions) {
        var out = "";
        var chars = Array.from(label);
        for (var i = 0; i < chars.length; i++) {
            var c = chars[i].replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
            out += positions.indexOf(i) !== -1 ? "<b><u>" + c + "</u></b>" : c;
        }
        return out;
    }
    function paletteWith(text) {
        commandText.text = text || "";
        commandPalette.open();
        commandPalette.search();
        commandText.forceActiveFocus();
        commandText.cursorPosition = commandText.text.length;
    }
    function openUrls(urls) {
        var opened = 0;
        for (var i = 0; i < urls.length; i++) {
            var path = slate.localPath(urls[i]);
            if (path.length) {
                root.send({"action": "open", "path": path});
                opened++;
            }
        }
        return opened;
    }
    onClosing: function (close) {
        close.accepted = false;
        slate.exit();
    }
    Connections {
        target: slate
        function onConfirmationRequested(id) {
            if (id === "quit") {
                quitDialog.open();
            } else {
                confirmDiscard.actionId = id;
                confirmDiscard.open();
            }
        }
        function onPathDialogOpenChanged() {
            if (!slate.pathDialogOpen)
                Qt.callLater(root.focusPane);
        }
        function onCloseDialogOpenChanged() {
            if (!slate.closeDialogOpen)
                Qt.callLater(root.focusPane);
        }
        function onFrameChanged() {
            root.updateGitFrame();
            if (root.gitCommitting)
                Qt.callLater(function () {
                    if (root.gitCommitting && !root.frame.git_busy) {
                        if (!root.frame.git_error && root.gitCommitDraft === root.gitSubmittedMessage)
                            root.gitCommitDraft = "";
                        root.gitCommitting = false;
                    }
                });
            // The core owns the picker; the dialog follows it.
            if (root.frame.picker && !root.frame.prompt) {
                if (!pickerDialog.visible) {
                    pickerDialog.open();
                    pickerQuery.text = root.frame.picker.query;
                }
            } else if (pickerDialog.visible) {
                pickerDialog.closing = true;
                pickerDialog.close();
                pickerDialog.closing = false;
            }
            var prompt = root.frame.prompt;
            if (prompt && prompt.kind === "settings") {
                Qt.callLater(function () {
                    settingsDialog.open();
                    root.send({"action": "dismiss_prompt"});
                });
                return;
            }
            if (prompt && prompt.kind === "close-tab") {
                Qt.callLater(function () {
                    if (root.frame.prompt && root.frame.prompt.kind === "close-tab")
                        slate.confirmCloseTab(root);
                });
                return;
            }
            if (prompt && ["open", "open-folder", "save-as"].indexOf(prompt.kind) !== -1) {
                Qt.callLater(function () {
                    if (root.frame.prompt && ["open", "open-folder", "save-as"].indexOf(root.frame.prompt.kind) !== -1)
                        slate.pickPath(root.frame.prompt.kind, root);
                });
                return;
            }
            if (prompt && choiceDialog.kinds.indexOf(prompt.kind) !== -1) {
                if (!choiceDialog.visible)
                    choiceDialog.open();
                return;
            } else if (choiceDialog.visible && (!prompt || choiceDialog.kinds.indexOf(prompt.kind) === -1)) {
                choiceDialog.close();
            }
            if (prompt && !editPrompt.visible) {
                editPrompt.open();
                promptInput.forceActiveFocus();
            } else if (!prompt && editPrompt.visible) {
                editPrompt.close();
                Qt.callLater(root.focusPane);
            }
            var paneKey = (root.frame.panes || []).map(function (pane) { return pane.id; }).join(",");
            if ((root.priorFocus !== root.frame.focus || root.priorPaneKey !== paneKey) && !anyDialogOpen()) {
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
    // Zoom follows desktop editors: Ctrl+= / Ctrl++ / Ctrl+- / Ctrl+0.
    Shortcut {
        sequences: ["Ctrl+=", "Ctrl++", StandardKey.ZoomIn]
        enabled: !anyDialogOpen()
        onActivated: root.invokeAction("zoom-in")
    }
    Shortcut {
        sequences: ["Ctrl+-", StandardKey.ZoomOut]
        enabled: !anyDialogOpen()
        onActivated: root.invokeAction("zoom-out")
    }
    Shortcut {
        sequence: "Ctrl+0"
        enabled: !anyDialogOpen()
        onActivated: root.invokeAction("zoom-reset")
    }
    Instantiator {
        model: root.frame.global_shortcuts || []
        delegate: Shortcut {
            required property string modelData
            sequence: modelData
            enabled: !root.frame.prompt && !anyDialogOpen()
            onActivated: root.send({"action": "shortcut", "chord": modelData})
        }
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
            color: Theme.backgroundColor
        }
        CommandMenu {
            objectName: "fileMenu"
            title: "&File"
            CommandMenuItem { actionId: "new"; label: "New File" }
            CommandMenuItem { actionId: "new-window"; label: "New Window" }
            CommandMenuItem { actionId: "open"; label: "Open File…" }
            CommandMenuItem { actionId: "open-folder"; label: "Open Folder…" }
            Menu {
                id: recentMenu
                objectName: "recentMenu"
                title: "Open Recent"
                enabled: (root.frame.recent || []).length > 0
                Instantiator {
                    model: root.frame.recent || []
                    delegate: Basic.MenuItem {
                        required property string modelData
                        required property int index
                        objectName: "recent_" + index
                        text: (index < 9 ? "&" + (index + 1) + "  " : "") + modelData.substring(modelData.lastIndexOf("/") + 1) + "   —   " + modelData
                        onTriggered: root.invokeAction("open-recent", modelData)
                    }
                    onObjectAdded: function (index, object) {
                        recentMenu.insertItem(index, object);
                    }
                    onObjectRemoved: function (index, object) {
                        recentMenu.removeItem(object);
                    }
                }
                MenuSeparator {}
                Basic.MenuItem {
                    text: "Clear Recent Files"
                    onTriggered: root.invokeAction("clear-recent")
                }
            }
            MenuSeparator {}
            CommandMenuItem { actionId: "save"; label: "Save" }
            CommandMenuItem { actionId: "save-as"; label: "Save As…" }
            CommandMenuItem { actionId: "save-all"; label: "Save All" }
            CommandMenuItem { actionId: "reload"; label: "Reload from Disk" }
            MenuSeparator {}
            CommandMenuItem { actionId: "print"; label: "Print…" }
            MenuSeparator {}
            CommandMenuItem { actionId: "close"; label: "Close Tab" }
            MenuSeparator {}
            CommandMenuItem { actionId: "settings"; label: "Settings…" }
            CommandMenuItem { actionId: "settings-reload"; label: "Reload Settings" }
            MenuSeparator {}
            CommandMenuItem { actionId: "quit"; label: "Quit" }
        }
        CommandMenu {
            objectName: "editMenu"
            title: "&Edit"
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
            CommandMenuItem { actionId: "search-in-files"; label: "Search in Files…" }
            CommandMenuItem { actionId: "replace-in-files"; label: "Replace in Files…" }
            CommandMenuItem { actionId: "undo-replace-in-files"; label: "Undo Replace in Files" }
            MenuSeparator {}
            CommandMenu {
                objectName: "selectionMenu"
                title: "Selection"
                CommandMenuItem { actionId: "add-next-occurrence"; label: "Add Next Occurrence" }
                CommandMenuItem { actionId: "select-all-occurrences"; label: "Select All Occurrences" }
                CommandMenuItem { actionId: "add-cursor-above"; label: "Add Cursor Above" }
                CommandMenuItem { actionId: "add-cursor-below"; label: "Add Cursor Below" }
            }
            CommandMenu {
                objectName: "codeMenu"
                title: "Code"
                CommandMenuItem { actionId: "trigger-completion"; label: "Complete" }
                CommandMenuItem { actionId: "format-document"; label: "Format Document" }
                CommandMenuItem { actionId: "rename-symbol"; label: "Rename Symbol…" }
                CommandMenuItem { actionId: "code-actions"; label: "Code Actions…" }
                MenuSeparator {}
                CommandMenuItem { actionId: "fold"; label: "Fold" }
                CommandMenuItem { actionId: "unfold"; label: "Unfold" }
                CommandMenuItem { actionId: "fold-all"; label: "Fold All" }
                CommandMenuItem { actionId: "unfold-all"; label: "Unfold All" }
            }
            MenuSeparator {}
            CommandMenuItem { actionId: "indent" }
            CommandMenuItem { actionId: "outdent" }
            CommandMenuItem { actionId: "justify" }
            CommandMenuItem { actionId: "insert-file" }
            MenuSeparator {}
            CommandMenuItem { actionId: "spell-check" }
            CommandMenuItem { actionId: "word-count" }
                MenuSeparator {}
                CommandMenu {
                objectName: "documentMenu"
                title: "Document"
                CommandMenu {
                    id: encodingMenu
                    objectName: "encodingMenu"
                    title: "Encoding"
                    Instantiator {
                        model: slate.encodings
                        delegate: CommandMenuItem {
                            required property string modelData
                            actionId: "set-encoding"
                            argument: modelData
                            label: modelData
                            checkable: true
                            checked: (root.frame.location || "").indexOf(" · " + modelData) !== -1
                        }
                        onObjectAdded: function (index, object) {
                            encodingMenu.insertItem(index, object);
                        }
                        onObjectRemoved: function (index, object) {
                            encodingMenu.removeItem(object);
                        }
                    }
                    MenuSeparator {}
                    CommandMenuItem { actionId: "toggle-bom"; label: "Byte-Order Mark" }
                }
                CommandMenu {
                    id: reopenMenu
                    title: "Reopen with Encoding"
                    Instantiator {
                        model: slate.encodings
                        delegate: CommandMenuItem {
                            required property string modelData
                            actionId: "reopen-encoding"
                            argument: modelData
                            label: modelData
                        }
                        onObjectAdded: function (index, object) {
                            reopenMenu.insertItem(index, object);
                        }
                        onObjectRemoved: function (index, object) {
                            reopenMenu.removeItem(object);
                        }
                    }
                }
                CommandMenu {
                    title: "Line Endings"
                    CommandMenuItem { actionId: "set-line-ending"; argument: "lf"; label: "Unix (LF)"; checkable: true; checked: / LF( ·|$)/.test(root.frame.location || "") }
                    CommandMenuItem { actionId: "set-line-ending"; argument: "crlf"; label: "Windows (CRLF)"; checkable: true; checked: / CRLF( ·|$)/.test(root.frame.location || "") }
                    CommandMenuItem { actionId: "set-line-ending"; argument: "cr"; label: "Classic Mac (CR)"; checkable: true; checked: / CR( ·|$)/.test(root.frame.location || "") }
                }
                MenuSeparator {}
                CommandMenuItem { actionId: "toggle-read-only"; label: "Read Only" }
                CommandMenuItem { actionId: "toggle-auto-reload"; label: "Reload Files Changed on Disk"; checkable: true; checked: !!root.settings.auto_reload }
            }
        }
        CommandMenu {
            objectName: "viewMenu"
            title: "&View"
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
            CommandMenuItem { actionId: "problems"; label: "Problems…" }
            CommandMenuItem { actionId: "open-documents"; label: "Open Documents…" }
            CommandMenuItem { actionId: "language-servers"; label: "Language Servers" }
            MenuSeparator {}
            CommandMenuItem { actionId: "toggle-soft-wrap"; label: "Word Wrap"; checkable: true; checked: !!root.settings.soft_wrap }
            CommandMenuItem { actionId: "toggle-whitespace"; label: "Show Whitespace"; checkable: true; checked: !!root.settings.show_whitespace }
            CommandMenuItem { actionId: "toggle-line-numbers"; label: "Line Numbers"; checkable: true; checked: !!root.settings.line_numbers }
            CommandMenuItem { actionId: "toggle-minimap"; label: "Minimap"; checkable: true; checked: !!root.settings.minimap }
            MenuSeparator {}
            CommandMenuItem { actionId: "zoom-in"; label: "Zoom In" }
            CommandMenuItem { actionId: "zoom-out"; label: "Zoom Out" }
            CommandMenuItem { actionId: "zoom-reset"; label: "Reset Zoom" }
            MenuSeparator {}
            CommandMenu {
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
        CommandMenu {
            objectName: "goMenu"
            title: "&Go"
            CommandMenuItem { actionId: "quick-open"; label: "Go to File…" }
            CommandMenuItem { actionId: "go-to-symbol"; label: "Go to Symbol…" }
            CommandMenuItem { actionId: "workspace-symbols"; label: "Workspace Symbols…" }
            CommandMenuItem { actionId: "prompt-goto" }
            MenuSeparator {}
            CommandMenuItem { actionId: "go-to-definition"; label: "Go to Definition" }
            CommandMenuItem { actionId: "find-references"; label: "Find References" }
            CommandMenuItem { actionId: "hover"; label: "Show Information" }
            CommandMenuItem { actionId: "go-to-bracket"; label: "Matching Bracket" }
            MenuSeparator {}
            CommandMenuItem { actionId: "go-back"; label: "Back" }
            CommandMenuItem { actionId: "go-forward"; label: "Forward" }
            CommandMenuItem { actionId: "next-problem"; label: "Next Problem" }
            CommandMenuItem { actionId: "previous-problem"; label: "Previous Problem" }
            CommandMenuItem { actionId: "find-next" }
            CommandMenuItem { actionId: "find-previous" }
            MenuSeparator {}
            CommandMenuItem { actionId: "next-tab" }
            CommandMenuItem { actionId: "previous-tab" }
            CommandMenuItem { actionId: "next-pane" }
        }
        CommandMenu {
            objectName: "runMenu"
            title: "&Run"
            CommandMenuItem { actionId: "debug-start"; label: root.frame.debugging ? "Continue" : "Start Debugging" }
            CommandMenuItem { actionId: "debug-step-over"; label: "Step Over" }
            CommandMenuItem { actionId: "debug-step-into"; label: "Step Into" }
            CommandMenuItem { actionId: "debug-step-out"; label: "Step Out" }
            CommandMenuItem { actionId: "debug-pause"; label: "Pause" }
            CommandMenuItem { actionId: "debug-stop"; label: "Stop Debugging" }
            CommandMenuItem { actionId: "debug-evaluate"; label: "Evaluate…" }
            CommandMenuItem { actionId: "debug-call-stack"; label: "Call Stack…" }
            MenuSeparator {}
            CommandMenuItem { actionId: "toggle-breakpoint"; label: "Toggle Breakpoint" }
            CommandMenuItem { actionId: "breakpoints"; label: "Breakpoints…" }
            CommandMenuItem { actionId: "clear-breakpoints"; label: "Remove All Breakpoints" }
            MenuSeparator {}
            CommandMenuItem { actionId: "run-build-task"; label: "Run Build Task" }
            CommandMenuItem { actionId: "run-task"; label: "Run Task…" }
            CommandMenuItem { actionId: "stop-task"; label: "Stop Tasks" }
            MenuSeparator {}
            // Terminals share this menu so the menubar fits narrow windows.
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
        // Drop files from a file manager to open them.
        DropArea {
            anchors.fill: parent
            keys: ["text/uri-list"]
            onDropped: function (drop) {
                if (drop.hasUrls && root.openUrls(drop.urls) > 0)
                    drop.acceptProposedAction();
            }
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
                readonly property bool isEditor: paneData.kind === "editor"
                readonly property bool showMinimap: isEditor && !!root.settings.minimap && width > 360
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
                color: Theme.backgroundColor
                border.color: root.frame.focus === paneId ? Theme.highlightColor : Theme.disabledTextColor
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
                    spacing: Theme.smallSpacing
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
                                        foregroundColor: fileTab.modelData.active ? Theme.highlightedTextColor : Theme.textColor
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
                    width: Math.max(0, parent.width - 2 - (panel.showMinimap ? root.minimapWidth : 0) - (editorScroll.visible ? editorScroll.width : 0))
                    height: Math.max(0, parent.height - root.paneHeaderHeight - 3 - (horizontalScroll.visible ? horizontalScroll.height : 0))
                    visible: panel.paneData.kind === "editor" || panel.paneData.kind === "terminal"
                    paneId: panel.paneId
                    Accessible.role: panel.paneData.kind === "editor" ? Accessible.EditableText : Accessible.Terminal
                    Accessible.name: (panel.paneData.tabs || []).filter(function (t) {
                        return t.active;
                    }).map(function (t) {
                        return t.title;
                    }).join("")
                    Accessible.description: panel.paneData.read_only ? "Read-only document" : (panel.paneData.kind === "editor" ? "Text editor" : "Terminal")
                    onContextMenuRequested: function (x, y) {
                        root.send({"action": "focus", "pane": panel.paneId});
                        if (panel.paneData.kind === "editor") {
                            editorMenu.commandPane = panel.paneId;
                            editorMenu.popup(grid, x, y);
                        } else {
                            paneMenu.popup(grid, x, y);
                        }
                    }
                }
                Minimap {
                    id: minimap
                    objectName: "minimap_" + panel.paneId
                    visible: panel.showMinimap
                    x: grid.x + grid.width
                    y: grid.y
                    width: root.minimapWidth
                    height: grid.height
                    paneId: panel.paneId
                    editor: panel.paneData.editor || ({})
                    visibleRows: panel.paneData.rows || 0
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
                        root.sendLater({
                            "action": "scroll_to",
                            "pane": panel.paneId,
                            "line": Math.round(position * panel.paneData.editor.line_count)
                        })
                    Accessible.name: "Document scroll position"
                }
                ScrollBar {
                    id: horizontalScroll
                    objectName: "horizontalScroll_" + panel.paneId
                    readonly property int contentColumns: minimap.widest
                    readonly property int scrollColumn: panel.paneData.editor ? panel.paneData.editor.left : 0
                    visible: panel.isEditor && !root.settings.soft_wrap && contentColumns > panel.paneData.cols
                    x: grid.x
                    y: grid.y + grid.height
                    width: grid.width
                    orientation: Qt.Horizontal
                    policy: ScrollBar.AsNeeded
                    size: Math.min(1, panel.paneData.cols / Math.max(1, contentColumns))
                    position: scrollColumn / Math.max(1, contentColumns)
                    onPositionChanged: if (pressed)
                        root.sendLater({
                            "action": "scroll_columns",
                            "pane": panel.paneId,
                            "delta": Math.round(position * contentColumns) - scrollColumn
                        })
                    Accessible.name: "Horizontal scroll position"
                }
                // Completions and hover text sit at their editor cell.
                Rectangle {
                    id: completionBox
                    objectName: "completion_" + panel.paneId
                    readonly property var info: root.frame.completion && root.frame.completion.pane === panel.paneId ? root.frame.completion : null
                    readonly property real rowHeight: Math.max(slate.cellHeight + 4, uiMetrics.height + 6)
                    readonly property real anchorY: grid.y + ((info ? info.row : 0) + 1) * slate.cellHeight - grid.scrollPixels
                    visible: !!info && panel.isEditor
                    z: 20
                    width: Math.min(panel.width - 8, 420)
                    height: Math.min(info ? info.items.length : 0, 10) * rowHeight + 4
                    x: Math.max(2, Math.min(grid.x + (info ? info.col : 0) * slate.cellWidth, panel.width - width - 4))
                    y: anchorY + height <= panel.height - 4 ? anchorY : Math.max(grid.y, anchorY - slate.cellHeight - height)
                    color: Theme.viewBackgroundColor
                    border.color: Theme.disabledTextColor
                    radius: 3
                    ListView {
                        id: completionList
                        objectName: "completionList_" + panel.paneId
                        anchors.fill: parent
                        anchors.margins: 2
                        clip: true
                        model: completionBox.info ? completionBox.info.items : []
                        currentIndex: completionBox.info ? completionBox.info.selected : -1
                        onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            width: completionList.width
                            height: completionBox.rowHeight
                            color: index === completionList.currentIndex ? Theme.highlightColor : "transparent"
                            RowLayout {
                                anchors.fill: parent
                                anchors.leftMargin: Theme.smallSpacing
                                anchors.rightMargin: Theme.smallSpacing
                                Label {
                                    textFormat: Text.PlainText
                                    text: modelData.label
                                    color: index === completionList.currentIndex ? Theme.highlightedTextColor : Theme.textColor
                                    Layout.fillWidth: true
                                    elide: Text.ElideRight
                                }
                                Label {
                                    textFormat: Text.PlainText
                                    text: modelData.detail || modelData.kind
                                    color: index === completionList.currentIndex ? Theme.highlightedTextColor : Theme.disabledTextColor
                                    Layout.maximumWidth: completionList.width * 0.45
                                    elide: Text.ElideRight
                                }
                            }
                            MouseArea {
                                anchors.fill: parent
                                onClicked: root.send({"action": "completion_accept", "index": index})
                            }
                        }
                    }
                }
                Rectangle {
                    id: hoverBox
                    objectName: "hover_" + panel.paneId
                    readonly property var info: root.frame.hover && root.frame.hover.pane === panel.paneId ? root.frame.hover : null
                    readonly property real anchorY: grid.y + ((info ? info.row : 0) + 1) * slate.cellHeight - grid.scrollPixels
                    visible: !!info && panel.isEditor
                    z: 20
                    // Text taller than half the pane scrolls, with room kept for the bar.
                    readonly property bool overflowing: hoverText.implicitHeight + 2 * Theme.largeSpacing > panel.height / 2
                    readonly property real barRoom: overflowing ? 12 : 0
                    width: Math.min(panel.width - 8, hoverText.implicitWidth + 2 * Theme.largeSpacing + barRoom, 560)
                    height: Math.min(panel.height / 2, hoverText.implicitHeight + 2 * Theme.largeSpacing)
                    x: Math.max(2, Math.min(grid.x + (info ? info.col : 0) * slate.cellWidth, panel.width - width - 4))
                    y: anchorY + height <= panel.height - 4 ? anchorY : Math.max(grid.y, anchorY - slate.cellHeight - height)
                    color: Theme.viewBackgroundColor
                    border.color: Theme.disabledTextColor
                    radius: 3
                    clip: true
                    // Long hover text scrolls instead of being cut off.
                    Flickable {
                        anchors.fill: parent
                        anchors.margins: Theme.largeSpacing
                        anchors.rightMargin: Theme.largeSpacing + hoverBox.barRoom
                        contentWidth: width
                        contentHeight: hoverText.implicitHeight
                        clip: true
                        boundsBehavior: Flickable.StopAtBounds
                        ScrollBar.vertical: ScrollBar {
                            parent: hoverBox
                            anchors.top: hoverBox.top
                            anchors.bottom: hoverBox.bottom
                            anchors.right: hoverBox.right
                            anchors.margins: 2
                            policy: hoverBox.overflowing ? ScrollBar.AlwaysOn : ScrollBar.AlwaysOff
                        }
                        Label {
                            id: hoverText
                            objectName: "hoverText_" + panel.paneId
                            width: Math.min(implicitWidth, 560 - 2 * Theme.largeSpacing - hoverBox.barRoom)
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            text: hoverBox.info ? hoverBox.info.text : ""
                        }
                    }
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
                    commandInfoFor: function (id, row) { return root.gitCommandInfo(id, panel.paneId, row); }
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
                        root.invokeAction("commit", message, panel.paneId);
                    }
                    onActionRequested: function (action) {
                        root.send(action);
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
                    // One shared hint for the whole list instead of one per row.
                    Hint {
                        id: rowHint
                        property Item target: browser
                        anchorItem: target
                        visible: false
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
                            color: fileDelegate.highlighted ? Theme.highlightedTextColor : Theme.textColor
                        }
                        background: Rectangle {
                            color: fileDelegate.highlighted ? Theme.highlightColor : fileDelegate.hovered ? Theme.alternateBackgroundColor : "transparent"
                        }
                        onHoveredChanged: {
                            if (hovered) {
                                rowHint.target = fileDelegate;
                                rowHint.text = fileDelegate.modelData.path;
                                rowHint.visible = true;
                            } else if (rowHint.target === fileDelegate) {
                                rowHint.visible = false;
                            }
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
                    anchors.fill: browser
                    acceptedButtons: Qt.RightButton
                    enabled: panel.paneData.kind === "files"
                    onClicked: function (mouse) {
                        var index = browser.indexAt(mouse.x, mouse.y + browser.contentY);
                        if (index >= 0)
                            root.send({
                                "action": "click",
                                "pane": panel.paneId,
                                "row": index,
                                "col": 0
                            });
                        root.send({
                            "action": "focus",
                            "pane": panel.paneId
                        });
                        paneMenu.popup();
                    }
                }
                CommandMenu {
                    id: paneMenu
                    objectName: "paneMenu_" + panel.paneId
                    x: Math.max(0, panel.width - width - 2)
                    y: root.paneHeaderHeight + 2
                    CommandMenuItem {
                        actionId: "save"
                        commandPane: panel.paneId
                        label: "Save document"
                        visible: panel.paneData.kind === "editor"
                    }
                    CommandMenuItem {
                        actionId: "prompt-find"
                        commandPane: panel.paneId
                        label: "Find…"
                        visible: panel.paneData.kind === "editor"
                    }
                    CommandMenuItem {
                        actionId: "terminal"
                        commandPane: panel.paneId
                        label: "New terminal"
                        visible: panel.paneData.kind === "terminal"
                    }
                    CommandMenuItem {
                        actionId: "split-right"
                        commandPane: panel.paneId
                        label: "Split right"
                    }
                    CommandMenuItem {
                        actionId: "split-down"
                        commandPane: panel.paneId
                        label: "Split below"
                    }
                    MenuSeparator {}
                    CommandMenuItem {
                        actionId: "copy"
                        commandPane: panel.paneId
                        label: "Copy selection"
                    }
                    CommandMenuItem {
                        actionId: "paste"
                        commandPane: panel.paneId
                    }
                    MenuSeparator {}
                    CommandMenuItem {
                        actionId: "files"
                        commandPane: panel.paneId
                        label: "Files view"
                    }
                    CommandMenuItem {
                        actionId: "git"
                        commandPane: panel.paneId
                        label: "Git view"
                    }
                    CommandMenuItem {
                        actionId: "editor"
                        commandPane: panel.paneId
                        label: "Editor view"
                    }
                    CommandMenuItem {
                        actionId: "terminal"
                        commandPane: panel.paneId
                        label: "Terminal view"
                    }
                    MenuSeparator {}
                    CommandMenuItem {
                        actionId: "close-pane"
                        commandPane: panel.paneId
                        label: "Close pane"
                    }
                    CommandMenuItem {
                        actionId: "move-pane"
                        commandPane: panel.paneId
                        label: "Move or swap pane…"
                    }
                    CommandMenuItem {
                        actionId: "toggle-workspace"
                        commandPane: panel.paneId
                        label: "Expand/collapse workspace"
                    }
                    MenuSeparator {}
                    CommandMenuItem {
                        actionId: "stage"
                        commandPane: panel.paneId
                        label: "Stage selected"
                        visible: panel.paneData.kind === "git"
                    }
                    CommandMenuItem {
                        actionId: "unstage"
                        commandPane: panel.paneId
                        label: "Unstage selected"
                        visible: panel.paneData.kind === "git"
                    }
                    CommandMenuItem {
                        actionId: "diff"
                        commandPane: panel.paneId
                        label: "View diff"
                        visible: panel.paneData.kind === "git"
                    }
                    CommandMenuItem {
                        actionId: "commit"
                        commandPane: panel.paneId
                        label: "Commit…"
                        visible: panel.paneData.kind === "git"
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
                color: drag.containsMouse ? Theme.highlightColor : Theme.disabledTextColor
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
                            root.sendLater({
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
    // The editor's right-click menu.
    CommandMenu {
        id: editorMenu
        objectName: "editorMenu"
        property int commandPane: 0
        CommandMenuItem { actionId: "undo"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "redo"; commandPane: editorMenu.commandPane }
        MenuSeparator {}
        CommandMenuItem { actionId: "cut"; label: "Cut"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "copy"; label: "Copy"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "paste"; label: "Paste"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "select-all"; label: "Select All"; commandPane: editorMenu.commandPane }
        MenuSeparator {}
        CommandMenuItem { actionId: "go-to-definition"; label: "Go to Definition"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "find-references"; label: "Find References"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "rename-symbol"; label: "Rename Symbol…"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "code-actions"; label: "Code Actions…"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "format-document"; label: "Format Document"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "toggle-breakpoint"; label: "Toggle Breakpoint"; commandPane: editorMenu.commandPane }
        MenuSeparator {}
        CommandMenuItem { actionId: "prompt-find"; label: "Find…"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "prompt-goto"; label: "Go to Line…"; commandPane: editorMenu.commandPane }
        MenuSeparator {}
        CommandMenuItem { actionId: "split-right"; commandPane: editorMenu.commandPane }
        CommandMenuItem { actionId: "split-down"; commandPane: editorMenu.commandPane }
    }
    footer: Basic.ToolBar {
        background: Rectangle {
            color: Theme.backgroundColor
        }
        implicitHeight: statusRow.implicitHeight + 2 * Theme.smallSpacing
        RowLayout {
            id: statusRow
            anchors.fill: parent
            anchors.margins: Theme.smallSpacing
            Label {
                id: statusLabel
                objectName: "statusLabel"
                textFormat: Text.PlainText
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                // Status messages and errors; key hints when there is no news.
                text: root.frame.status || root.frame.hints || ""
                color: root.statusIsError ? Theme.negativeTextColor : Theme.textColor
                font.bold: root.statusIsError
                elide: Text.ElideRight
                Accessible.role: Accessible.StatusBar
                Hint {
                    anchorItem: statusLabel
                    visible: statusHover.hovered
                    text: statusLabel.text + (root.frame.hints ? "\n" + root.frame.hints : "")
                }
                HoverHandler {
                    id: statusHover
                }
            }
            Row {
                objectName: "debugToolbar"
                visible: !!root.frame.debugging
                spacing: 2
                Repeater {
                    model: [
                        {"id": "debug-start", "text": "▶", "tip": "Continue"},
                        {"id": "debug-step-over", "text": "↷", "tip": "Step over"},
                        {"id": "debug-step-into", "text": "↓", "tip": "Step into"},
                        {"id": "debug-step-out", "text": "↑", "tip": "Step out"},
                        {"id": "debug-pause", "text": "❚❚", "tip": "Pause"},
                        {"id": "debug-stop", "text": "■", "tip": "Stop debugging"}
                    ]
                    delegate: ActionButton {
                        required property var modelData
                        objectName: "debug_" + modelData.id
                        text: modelData.text
                        tip: modelData.tip
                        flat: true
                        enabled: modelData.id === "debug-pause" ? !root.frame.debug_paused
                                 : modelData.id === "debug-stop" || !!root.frame.debug_paused
                        onClicked: root.invokeAction(modelData.id)
                    }
                }
            }
            Label {
                objectName: "activity"
                textFormat: Text.PlainText
                text: root.frame.activity || ""
                visible: text.length > 0
                color: Theme.disabledTextColor
                Layout.maximumWidth: root.width * 0.25
                elide: Text.ElideRight
            }
            Label {
                objectName: "problemHere"
                textFormat: Text.PlainText
                text: root.frame.problem || ""
                visible: text.length > 0
                color: text.indexOf("error") === 0 ? Theme.negativeTextColor : Theme.neutralTextColor
                Layout.maximumWidth: root.width * 0.3
                elide: Text.ElideRight
            }
            ActionButton {
                objectName: "problemCount"
                readonly property var counts: root.frame.problems || [0, 0]
                visible: counts[0] + counts[1] > 0
                text: "✖ " + counts[0] + "  ⚠ " + counts[1]
                tip: "Problems"
                flat: true
                onClicked: root.invokeAction("problems")
            }
            ActionButton {
                objectName: "restrictedMode"
                visible: root.frame.trusted === false
                text: "Restricted"
                tip: "Project tools do not run in this folder. Click to trust it."
                flat: true
                onClicked: root.invokeAction("trust-workspace")
            }
            Label {
                objectName: "branch"
                textFormat: Text.PlainText
                text: root.frame.git_branch ? "⎇ " + root.frame.git_branch : ""
                visible: text.length > 0
                Layout.maximumWidth: root.width * 0.2
                elide: Text.ElideRight
            }
            Label {
                textFormat: Text.PlainText
                text: root.frame.location || ""
                visible: text.length > 0
                Layout.maximumWidth: root.width * 0.45
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
                "stage-group": "Stage a change group",
                "settings": "Settings",
                "insert-file": "Insert file",
                "set-encoding": "Save with encoding",
                "reopen-encoding": "Reopen with encoding",
                "set-line-ending": "Line endings",
                "open-recent": "Open recent file",
                "rename-symbol": "Rename symbol",
                "project-replace": "Replace in files",
                "debug-evaluate": "Evaluate expression",
                "debug-program": "Start debugging"
            })[prompt.kind] || "Input"
        // Find and replace sit at the bottom without dimming the text, so
        // the matches stay visible; other prompts are centred.
        readonly property bool searching: prompt.kind === "find" || prompt.kind === "replace"
        x: Math.round((parent.width - width) / 2)
        y: searching ? parent.height - height - Theme.largeSpacing : Math.round((parent.height - height) / 2)
        width: Math.min(root.width - 40, searching ? 720 : 560)
        height: Math.min(root.height - 40, implicitHeight)
        modal: true
        dim: !searching
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
                spacing: Theme.smallSpacing
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
                            "stage-group": "Change group, e.g. Untracked",
                            "settings": "Option and value, e.g. indent-width 4",
                            "insert-file": "Path of the file to insert",
                            "set-encoding": "utf-8, utf-16le, windows-1252, shift_jis…",
                            "reopen-encoding": "utf-8, latin1, shift_jis…",
                            "set-line-ending": "lf, crlf or cr",
                            "open-recent": "Path or list number",
                            "rename-symbol": "New name",
                            "project-replace": "Replacement text",
                            "debug-evaluate": "Expression, e.g. count * 2",
                            "debug-program": "Path of the program to debug"
                        })[editPrompt.prompt.kind] || "Find text"
                    onTextEdited: editPrompt.update()
                    onAccepted: root.send({
                        "action": "submit_prompt"
                    })
                    Keys.onPressed: function (event) {
                        // Alt+C / Alt+W toggle the options, as in the terminal.
                        if ((event.modifiers & Qt.AltModifier) && editPrompt.searching
                                && (event.key === Qt.Key_C || event.key === Qt.Key_W)) {
                            if (event.key === Qt.Key_C)
                                matchCase.checked = !matchCase.checked;
                            else
                                wholeWord.checked = !wholeWord.checked;
                            editPrompt.update();
                            event.accepted = true;
                        }
                    }
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
                    spacing: Theme.smallSpacing
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
                    color: root.statusIsError ? Theme.negativeTextColor : Theme.textColor
                    wrapMode: Text.Wrap
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Theme.smallSpacing
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
    // Questions the core asks with a fixed set of answers.
    Dialog {
        id: choiceDialog
        objectName: "choiceDialog"
        enter: Transition {}
        exit: Transition {}
        readonly property var kinds: ["save-read-only", "save-elevated", "reload-changed", "file-changed", "quit", "trust", "confirm-replace"]
        readonly property var prompt: root.frame.prompt || ({ "kind": "", "input": "" })
        readonly property var copy: ({
                "save-read-only": ["Read-only file", "%1 is read-only. Overwrite it anyway?", "Overwrite", ""],
                "save-elevated": ["Permission denied", "You do not have permission to write %1. Save it with administrator rights?", "Save as Administrator", ""],
                "reload-changed": ["Reload from disk", "Reload %1 from disk and discard your unsaved changes?", "Reload", ""],
                "file-changed": ["File changed on disk", "%1 changed on disk while you have unsaved changes.", "Reload from Disk", "Keep My Version"],
                "quit": ["Unsaved changes", "Save changes before quitting? (%1)", "Save All and Quit", "Discard and Quit"],
                "confirm-replace": ["Replace in files", "Replace every search result in %1? Open documents change in the editor and stay unsaved.", "Replace All", ""],
                "trust": ["Trust this folder?", "%1. Trusting lets this folder's language servers, tasks, formatters and Git hooks run.", "Trust Folder", ""]
            })[prompt.kind] || ["", "", "OK", ""]
        title: copy[0]
        anchors.centerIn: parent
        width: Math.min(root.width - 40, 520)
        modal: true
        closePolicy: Popup.CloseOnEscape
        onRejected: root.send({"action": "dismiss_prompt"})
        onClosed: Qt.callLater(root.focusPane)
        contentItem: ColumnLayout {
            spacing: Theme.largeSpacing
            Label {
                objectName: "choiceMessage"
                Layout.fillWidth: true
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                text: choiceDialog.copy[1].replace("%1", choiceDialog.prompt.input)
            }
            Flow {
                Layout.fillWidth: true
                spacing: Theme.smallSpacing
                layoutDirection: Qt.RightToLeft
                ActionButton {
                    objectName: "choiceAccept"
                    text: choiceDialog.copy[2]
                    highlighted: true
                    onClicked: {
                        choiceDialog.close();
                        root.send({"action": "submit_prompt"});
                    }
                }
                ActionButton {
                    objectName: "choiceAlternative"
                    visible: choiceDialog.copy[3].length > 0
                    text: choiceDialog.copy[3]
                    onClicked: {
                        choiceDialog.close();
                        root.send({"action": "submit_prompt", "all": true});
                    }
                }
                ActionButton {
                    objectName: "choiceCancel"
                    text: choiceDialog.prompt.kind === "file-changed" ? "Decide Later" : "Cancel"
                    onClicked: choiceDialog.reject()
                }
            }
        }
    }
    // Files, symbols, search results, problems: lists the core filters.
    Dialog {
        id: pickerDialog
        enter: Transition {}
        exit: Transition {}
        objectName: "pickerDialog"
        readonly property var info: root.frame.picker || ({"title": "", "items": [], "selected": 0, "total": 0, "message": "", "kind": ""})
        property bool closing: false
        title: info.title
        anchors.centerIn: parent
        width: Math.min(root.width - 40, 760)
        height: Math.min(root.height - 40, 520)
        modal: true
        onOpened: pickerQuery.forceActiveFocus()
        onClosed: {
            if (!closing && root.frame.picker)
                root.send({"action": "picker_close"});
            Qt.callLater(root.focusPane);
        }
        contentItem: ColumnLayout {
            TextField {
                id: pickerQuery
                objectName: "pickerQuery"
                Layout.fillWidth: true
                placeholderText: pickerDialog.info.kind === "search" ? "Search the workspace…" : "Type to filter…"
                onTextEdited: root.send({"action": "picker_query", "query": text})
                onAccepted: root.send({"action": "picker_accept"})
                Keys.onDownPressed: root.send({"action": "picker_move", "delta": 1})
                Keys.onUpPressed: root.send({"action": "picker_move", "delta": -1})
                Keys.onPressed: function (event) {
                    if (event.key === Qt.Key_PageDown) {
                        root.send({"action": "picker_move", "delta": 10});
                        event.accepted = true;
                    } else if (event.key === Qt.Key_PageUp) {
                        root.send({"action": "picker_move", "delta": -10});
                        event.accepted = true;
                    } else if (event.key === Qt.Key_H && (event.modifiers & Qt.ControlModifier) && pickerDialog.info.kind === "search") {
                        root.send({"action": "action", "name": "replace-in-files", "argument": ""});
                        event.accepted = true;
                    } else if ((event.modifiers & Qt.AltModifier) && pickerDialog.info.kind === "search"
                               && [Qt.Key_C, Qt.Key_W, Qt.Key_R].indexOf(event.key) !== -1) {
                        // Alt+C / Alt+W / Alt+R toggle case, whole word and regex, as in the terminal.
                        root.send({"action": "picker_option", "name": event.key === Qt.Key_C ? "case" : event.key === Qt.Key_W ? "word" : "regex"});
                        event.accepted = true;
                    }
                }
            }
            Flow {
                Layout.fillWidth: true
                spacing: Theme.smallSpacing
                visible: pickerDialog.info.kind === "search"
                CheckBox {
                    objectName: "pickerCase"
                    text: "Match case"
                    checked: !!pickerDialog.info.case_sensitive
                    onClicked: root.send({"action": "picker_option", "name": "case"})
                }
                CheckBox {
                    objectName: "pickerWord"
                    text: "Whole word"
                    checked: !!pickerDialog.info.whole_word
                    onClicked: root.send({"action": "picker_option", "name": "word"})
                }
                CheckBox {
                    objectName: "pickerRegex"
                    text: "Regular expression"
                    checked: !!pickerDialog.info.regex
                    onClicked: root.send({"action": "picker_option", "name": "regex"})
                }
                ActionButton {
                    text: "Replace…"
                    onClicked: root.send({"action": "action", "name": "replace-in-files", "argument": ""})
                }
            }
            ListView {
                id: pickerResults
                objectName: "pickerResults"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: pickerDialog.info.items
                // Items are a window of the list starting at `first`.
                currentIndex: pickerDialog.info.selected - (pickerDialog.info.first || 0)
                onCurrentIndexChanged: if (currentIndex >= 0 && currentIndex < count) positionViewAtIndex(currentIndex, ListView.Contain)
                onAtYEndChanged: {
                    var info = pickerDialog.info;
                    if (atYEnd && count > 0 && (info.first || 0) + count < info.total)
                        root.send({"action": "picker_window", "first": (info.first || 0) + Math.floor(count / 2)});
                }
                onAtYBeginningChanged: {
                    var info = pickerDialog.info;
                    if (atYBeginning && (info.first || 0) > 0)
                        root.send({"action": "picker_window", "first": Math.max(0, (info.first || 0) - Math.floor(count / 2))});
                }
                ScrollBar.vertical: ScrollBar {}
                delegate: Basic.ItemDelegate {
                    id: pickerItem
                    required property var modelData
                    required property int index
                    width: pickerResults.width
                    highlighted: index === pickerResults.currentIndex
                    contentItem: RowLayout {
                        Label {
                            // Matched characters are emphasised, as in the terminal.
                            textFormat: Text.StyledText
                            text: root.marked(pickerItem.modelData.label, pickerItem.modelData.positions || [])
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            elide: Text.ElideMiddle
                            color: pickerItem.highlighted ? Theme.highlightedTextColor : Theme.textColor
                        }
                        Label {
                            textFormat: Text.PlainText
                            text: pickerItem.modelData.detail
                            Layout.maximumWidth: pickerResults.width * 0.45
                            elide: Text.ElideMiddle
                            color: pickerItem.highlighted ? Theme.highlightedTextColor : Theme.disabledTextColor
                        }
                    }
                    background: Rectangle {
                        color: pickerItem.highlighted ? Theme.highlightColor : "transparent"
                        radius: 4
                    }
                    onClicked: root.send({"action": "picker_accept", "index": (pickerDialog.info.first || 0) + index})
                }
                Label {
                    textFormat: Text.PlainText
                    anchors.centerIn: parent
                    visible: pickerResults.count === 0
                    text: pickerDialog.info.busy ? "Working…" : (pickerDialog.info.message || "Nothing found")
                }
            }
            Label {
                objectName: "pickerStatus"
                textFormat: Text.PlainText
                Layout.fillWidth: true
                elide: Text.ElideRight
                text: (pickerDialog.info.busy ? "Working… · " : "") + (pickerDialog.info.message || (pickerDialog.info.total === 1 ? "1 item" : pickerDialog.info.total + " items")) + " · ↑ ↓ select · Enter opens · Escape closes"
                color: Theme.disabledTextColor
            }
        }
    }
    Dialog {
        id: commandPalette
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
        // Results change with the query, not with every editor frame.
        property var results: []
        function search() {
            results = slate.commands(commandText.text);
        }
        function execute() {
            var raw = commandText.text.trim();
            if (raw.charAt(0) === ":") {
                commandPalette.close();
                slate.command(raw.substring(1));
                Qt.callLater(root.focusPane);
                return;
            }
            if (commandResults.currentIndex < 0 || commandResults.currentIndex >= results.length)
                return;
            var action = results[commandResults.currentIndex];
            if (!action.enabled)
                return;
            commandPalette.close();
            root.invokeAction(action.id);
        }
        onRejected: Qt.callLater(root.focusPane)
        contentItem: ColumnLayout {
            TextField {
                id: commandText
                objectName: "commandSearch"
                Layout.fillWidth: true
                placeholderText: "Search actions…"
                onTextChanged: {
                    commandResults.currentIndex = 0;
                    commandPalette.search();
                }
                onAccepted: commandPalette.execute()
                Keys.onDownPressed: commandResults.currentIndex = Math.min(commandResults.count - 1, commandResults.currentIndex + 1)
                Keys.onUpPressed: commandResults.currentIndex = Math.max(0, commandResults.currentIndex - 1)
            }
            ListView {
                id: commandResults
                objectName: "commandResults"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: commandPalette.results
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
                                color: commandResult.highlighted ? Theme.highlightedTextColor : Theme.textColor
                            }
                            Label {
                                textFormat: Text.PlainText
                                text: commandResult.modelData.shortcut
                                color: commandResult.highlighted ? Theme.highlightedTextColor : Theme.disabledTextColor
                            }
                        }
                        Label {
                            textFormat: Text.PlainText
                            text: commandResult.modelData.enabled ? commandResult.modelData.description : commandResult.modelData.reason
                            Layout.fillWidth: true
                            elide: Text.ElideRight
                            color: commandResult.highlighted ? Theme.highlightedTextColor : Theme.disabledTextColor
                        }
                    }
                    background: Rectangle {
                        color: commandResult.highlighted ? Theme.highlightColor : "transparent"
                        radius: 4
                    }
                    onClicked: {
                        commandResults.currentIndex = index;
                        commandPalette.execute();
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
                color: Theme.disabledTextColor
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
        width: Math.min(root.width - 40, 520)
        height: Math.min(root.height - 40, implicitHeight)
        modal: true
        standardButtons: Dialog.Close
        function configure(name, value) {
            root.send({"action": "configure", "name": name, "value": value});
        }
        contentItem: ScrollView {
            id: settingsScroll
            clip: true
            contentWidth: availableWidth
            leftPadding: Theme.largeSpacing
            rightPadding: Theme.largeSpacing
            topPadding: Theme.smallSpacing
            bottomPadding: Theme.smallSpacing
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
                            currentIndex: root.settings[modelData.field] === "editor-only" ? 0 : 1
                            onActivated: settingsDialog.configure(modelData.setting, currentIndex === 0 ? "editor-only" : "workspace")
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
                        text: "Font"
                        Layout.fillWidth: true
                    }
                    ComboBox {
                        id: fontFamily
                        objectName: "settingsFontFamily"
                        Layout.preferredWidth: 220
                        model: slate.fontFamilies
                        currentIndex: Math.max(0, model.indexOf(root.settings.font_family || ""))
                        displayText: currentIndex === 0 ? "System fixed-width font" : currentText
                        onActivated: settingsDialog.configure("font-family", currentIndex === 0 ? "" : currentText)
                    }
                    SpinBox {
                        objectName: "settingsFontSize"
                        from: 6
                        to: 72
                        value: root.settings.font_size || 11
                        onValueModified: settingsDialog.configure("font-size", value.toString())
                    }
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
                        value: root.settings.indent_width || 4
                        onValueModified: settingsDialog.configure("indent-width", value.toString())
                    }
                }
                Repeater {
                    model: [
                        {"label": "Insert spaces instead of tabs", "setting": "insert-spaces", "field": "insert_spaces"},
                        {"label": "Indent new lines automatically", "setting": "auto-indent", "field": "auto_indent"},
                        {"label": "Show line numbers", "setting": "line-numbers", "field": "line_numbers"},
                        {"label": "Wrap long lines", "setting": "soft-wrap", "field": "soft_wrap"},
                        {"label": "Show whitespace", "setting": "show-whitespace", "field": "show_whitespace"},
                        {"label": "Show minimap", "setting": "minimap", "field": "minimap"},
                        {"label": "Reload files changed on disk", "setting": "auto-reload", "field": "auto_reload"},
                        {"label": "Keep a backup (NAME~) when saving", "setting": "backup", "field": "backup"},
                        {"label": "Recover unsaved single files after a crash", "setting": "file-recovery", "field": "file_recovery"},
                        {"label": "Close brackets and quotes", "setting": "auto-close-brackets", "field": "auto_close_brackets"},
                        {"label": "Complete while typing", "setting": "complete-while-typing", "field": "complete_while_typing"},
                        {"label": "Enter accepts a completion", "setting": "accept-completion-on-enter", "field": "accept_completion_on_enter"},
                        {"label": "Format on save (trusted folders)", "setting": "format-on-save", "field": "format_on_save"},
                        {"label": "Hard wrap while typing", "setting": "hard-wrap", "field": "hard_wrap"},
                        {"label": "Terminal programs may set the clipboard", "setting": "terminal-clipboard", "field": "terminal_clipboard"}
                    ]
                    delegate: CheckBox {
                        required property var modelData
                        objectName: "settings_" + modelData.field
                        text: modelData.label
                        checked: !!root.settings[modelData.field]
                        onClicked: settingsDialog.configure(modelData.setting, checked.toString())
                    }
                }
                Repeater {
                    model: [
                        {"label": "Wrap column (justify and hard wrap)", "setting": "wrap-column", "field": "wrap_column", "from": 10, "to": 500},
                        {"label": "Terminal scrollback lines", "setting": "terminal-scrollback", "field": "terminal_scrollback", "from": 0, "to": 100000}
                    ]
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        Label {
                            textFormat: Text.PlainText
                            text: modelData.label
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            wrapMode: Text.Wrap
                        }
                        SpinBox {
                            objectName: "settings_" + modelData.field
                            from: modelData.from
                            to: modelData.to
                            editable: true
                            value: root.settings[modelData.field] || 0
                            onValueModified: settingsDialog.configure(modelData.setting, value.toString())
                        }
                    }
                }
                RowLayout {
                    Label {
                        textFormat: Text.PlainText
                        text: "Theme"
                        Layout.fillWidth: true
                    }
                    ComboBox {
                        model: ["auto", "dark", "light"]
                        currentIndex: model.indexOf(root.settings.theme || "auto")
                        onActivated: settingsDialog.configure("theme", currentText)
                    }
                }
                RowLayout {
                    Label {
                        textFormat: Text.PlainText
                        text: "Keymap"
                        Layout.fillWidth: true
                    }
                    ComboBox {
                        objectName: "settingsKeymap"
                        model: ["default", "nano"]
                        currentIndex: model.indexOf(root.settings.keymap || "default")
                        onActivated: settingsDialog.configure("keymap", currentText)
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
        objectName: "confirmDiscard"
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
            "id": actionId,
            "confirmed": true
        })
        onClosed: Qt.callLater(root.focusPane)
    }
    Dialog {
        id: quitDialog
        objectName: "quitDialog"
        enter: Transition {}
        exit: Transition {}
        title: "Unsaved documents"
        anchors.centerIn: parent
        modal: true
        closePolicy: Popup.CloseOnEscape
        onClosed: Qt.callLater(root.focusPane)
        width: Math.min(root.width - 40, 480)
        // Buttons wrap on narrow windows instead of overflowing.
        contentItem: ColumnLayout {
            spacing: Theme.largeSpacing
            Label {
                textFormat: Text.PlainText
                Layout.fillWidth: true
                text: "Save your changes before quitting? Discard closes Slate without saving."
                wrapMode: Text.Wrap
            }
            Flow {
                Layout.fillWidth: true
                spacing: Theme.smallSpacing
                layoutDirection: Qt.RightToLeft
                ActionButton {
                    objectName: "quitSave"
                    text: "Save All"
                    highlighted: true
                    onClicked: {
                        quitDialog.close();
                        root.send({"action": "save_all", "quit": true});
                    }
                }
                ActionButton {
                    objectName: "quitDiscard"
                    text: "Discard"
                    onClicked: {
                        quitDialog.close();
                        root.send({"action": "quit", "force": true, "confirmed": true});
                    }
                }
                ActionButton {
                    objectName: "quitCancel"
                    text: "Cancel"
                    onClicked: quitDialog.reject()
                }
            }
        }
    }
}
