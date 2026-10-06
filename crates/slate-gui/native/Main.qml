import QtQuick
import QtQuick.Controls
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
    property int priorFocus: -1
    // One measured header size is shared with Rust's editor/PTY viewport calculation.
    FontMetrics {
        id: uiMetrics
        font: root.font
    }
    readonly property int paneHeaderHeight: Math.ceil(Math.max(40, uiMetrics.height + 20))
    readonly property int fileRowHeight: Math.ceil(Math.max(28, uiMetrics.height + 12))
    function syncViewport() {
        slate.paneHeader(paneHeaderHeight + 3);
        slate.viewport(workspace.width, workspace.height);
    }
    onPaneHeaderHeightChanged: Qt.callLater(syncViewport)
    function send(action) {
        slate.send(action);
        slate.refresh();
        if (!frame.prompt && !palette.visible && !quitDialog.visible)
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
        for (var i = 0; i < panes.count; i++) {
            var item = panes.itemAt(i);
            if (item && item.paneId === frame.focus)
                item.focusContent();
        }
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
        function onFrameChanged() {
            if (root.frame.prompt && !editPrompt.visible) {
                editPrompt.open();
                promptInput.forceActiveFocus();
            } else if (!root.frame.prompt && editPrompt.visible) {
                editPrompt.close();
                Qt.callLater(root.focusPane);
            }
            if (root.priorFocus !== root.frame.focus) {
                root.priorFocus = root.frame.focus;
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
    header: ToolBar {
        id: mainToolbar
        objectName: "mainToolbar"
        implicitHeight: mainActions.implicitHeight + 2 * Kirigami.Units.smallSpacing
        RowLayout {
            id: mainActions
            objectName: "mainActions"
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            spacing: Kirigami.Units.smallSpacing
            ToolButton {
                id: menuButton
                objectName: "mainMenuButton"
                text: "Menu"
                onClicked: mainMenu.popup()
                ToolTip.visible: hovered
                ToolTip.text: "File, editing, and workspace commands"
                Menu {
                    id: mainMenu
                    y: menuButton.height
                    Menu {
                        title: "File"
                        MenuItem {
                            text: "Open…"
                            onTriggered: root.paletteWith("open ")
                        }
                        MenuItem {
                            text: "New document"
                            onTriggered: root.send({
                                    "action": "new"
                                })
                        }
                        MenuItem {
                            text: "Save"
                            onTriggered: root.send({
                                    "action": "save"
                                })
                        }
                        MenuItem {
                            text: "Save as…"
                            onTriggered: root.paletteWith("save-as ")
                        }
                        MenuSeparator {
                        }
                        MenuItem {
                            text: "Quit"
                            onTriggered: root.close()
                        }
                    }
                    Menu {
                        title: "Edit"
                        MenuItem {
                            text: "Undo"
                            onTriggered: root.send({
                                    "action": "undo"
                                })
                        }
                        MenuItem {
                            text: "Redo"
                            onTriggered: root.send({
                                    "action": "redo"
                                })
                        }
                        MenuSeparator {
                        }
                        MenuItem {
                            text: "Find…"
                            onTriggered: root.send({
                                    "action": "prompt",
                                    "kind": "find"
                                })
                        }
                        MenuItem {
                            text: "Replace…"
                            onTriggered: root.send({
                                    "action": "prompt",
                                    "kind": "replace"
                                })
                        }
                        MenuItem {
                            text: "Go to line…"
                            onTriggered: root.send({
                                    "action": "prompt",
                                    "kind": "goto"
                                })
                        }
                    }
                    Menu {
                        title: "Workspace"
                        MenuItem {
                            text: "Three panes"
                            onTriggered: root.send({
                                    "action": "preset",
                                    "name": "development"
                                })
                        }
                        MenuItem {
                            text: "Terminal below"
                            onTriggered: root.send({
                                    "action": "preset",
                                    "name": "bottom_terminal"
                                })
                        }
                        MenuItem {
                            text: "Editor only"
                            onTriggered: root.send({
                                    "action": "preset",
                                    "name": "minimal"
                                })
                        }
                        MenuItem {
                            text: "Focus next pane"
                            onTriggered: slate.key(Qt.Key_F6, "", 0)
                        }
                        MenuSeparator {
                        }
                        MenuItem {
                            text: "Save named layout…"
                            onTriggered: root.paletteWith("layout-save ")
                        }
                        MenuItem {
                            text: "Load named layout…"
                            onTriggered: root.paletteWith("layout-load ")
                        }
                        MenuSeparator {
                        }
                        MenuItem {
                            text: "Editor settings…"
                            onTriggered: root.paletteWith("set ")
                        }
                        MenuItem {
                            text: "Reload settings"
                            onTriggered: root.send({
                                    "action": "reload_settings"
                                })
                        }
                    }
                    MenuSeparator {
                    }
                    MenuItem {
                        text: "Commands…"
                        onTriggered: root.paletteWith("")
                    }
                }
            }
            ToolButton {
                id: openButton
                objectName: "openButton"
                text: "Open…"
                visible: mainToolbar.width > implicitWidth + saveButton.implicitWidth + commandsButton.implicitWidth + menuButton.implicitWidth + 6 * Kirigami.Units.smallSpacing
                onClicked: root.paletteWith("open ")
            }
            ToolButton {
                id: saveButton
                objectName: "saveButton"
                text: "Save"
                visible: mainToolbar.width > implicitWidth + commandsButton.implicitWidth + menuButton.implicitWidth + 5 * Kirigami.Units.smallSpacing
                onClicked: root.send({
                        "action": "save"
                    })
            }
            Item {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
            }
            ToolButton {
                id: commandsButton
                objectName: "commandsButton"
                text: "Commands"
                onClicked: root.paletteWith("")
                ToolTip.visible: hovered
                ToolTip.text: "Command palette (Ctrl+Shift+P or F1)"
            }
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
                    ListView {
                        id: tabs
                        objectName: "tabs_" + panel.paneId
                        Layout.fillWidth: true
                        Layout.minimumWidth: 0
                        Layout.fillHeight: true
                        orientation: ListView.Horizontal
                        boundsBehavior: Flickable.StopAtBounds
                        clip: true
                        model: panel.paneData.tabs
                        currentIndex: {
                            for (var i = 0; i < panel.paneData.tabs.length; i++)
                                if (panel.paneData.tabs[i].active)
                                    return i;
                            return 0;
                        }
                        onCurrentIndexChanged: Qt.callLater(function () {
                                tabs.positionViewAtIndex(tabs.currentIndex, ListView.Contain);
                            })
                        onWidthChanged: Qt.callLater(function () {
                                tabs.positionViewAtIndex(tabs.currentIndex, ListView.Contain);
                            })
                        ScrollBar.horizontal: ScrollBar {
                            policy: ScrollBar.AsNeeded
                        }
                        delegate: Button {
                            required property var modelData
                            required property int index
                            objectName: "tab_" + panel.paneId + "_" + index
                            text: modelData.title
                            height: root.paneHeaderHeight
                            width: Math.max(0, Math.min(tabs.width, 240, implicitWidth))
                            highlighted: modelData.active
                            font.bold: modelData.active
                            contentItem: Label {
                                text: parent.text
                                font: parent.font
                                horizontalAlignment: Text.AlignLeft
                                verticalAlignment: Text.AlignVCenter
                                elide: Text.ElideMiddle
                            }
                            ToolTip.visible: hovered
                            ToolTip.text: modelData.title
                            onClicked: {
                                root.send({
                                        "action": "switch_tab",
                                        "pane": panel.paneId,
                                        "index": index
                                    });
                                panel.focusContent();
                            }
                        }
                    }
                    ToolButton {
                        objectName: "paneActions_" + panel.paneId
                        text: "⋮"
                        Layout.preferredWidth: root.paneHeaderHeight
                        Layout.preferredHeight: root.paneHeaderHeight
                        ToolTip.visible: hovered
                        ToolTip.text: "Pane actions"
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
                    pane: panel.paneData
                }
                ListView {
                    id: browser
                    objectName: "browser_" + panel.paneId
                    x: 1
                    y: root.paneHeaderHeight + 2
                    width: Math.max(0, parent.width - 2)
                    height: Math.max(0, parent.height - root.paneHeaderHeight - 3)
                    visible: panel.paneData.kind === "files" || panel.paneData.kind === "git"
                    clip: true
                    model: panel.paneData.kind === "git" ? slate.git : slate.files
                    currentIndex: panel.paneData.selected || 0
                    ScrollBar.vertical: ScrollBar {
                    }
                    Keys.onPressed: function (event) {
                        slate.key(event.key, event.text, event.modifiers);
                        event.accepted = true;
                    }
                    delegate: ItemDelegate {
                        required property var modelData
                        required property int index
                        height: root.fileRowHeight
                        width: browser.width
                        contentItem: Label {
                            text: parent.text
                            font: parent.font
                            elide: Text.ElideMiddle
                            verticalAlignment: Text.AlignVCenter
                        }
                        ToolTip.visible: hovered
                        ToolTip.text: modelData.path
                        text: panel.paneData.kind === "git" ? modelData.status + "  " + modelData.path : (modelData.directory ? "▸  " : "   ") + modelData.name
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
                            if (panel.paneData.kind === "git")
                                root.send({
                                        "action": "git_diff",
                                        "path": modelData.path
                                    });
                            else
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
                    enabled: panel.paneData.kind !== "terminal"
                    onClicked: {
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
                    MenuSeparator {
                    }
                    MenuItem {
                        text: "Copy selection"
                        onTriggered: slate.copyClipboard()
                    }
                    MenuItem {
                        text: "Paste"
                        onTriggered: slate.pasteClipboard()
                    }
                    MenuSeparator {
                    }
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
                    MenuSeparator {
                    }
                    MenuItem {
                        text: "Close pane"
                        onTriggered: root.send({
                                "action": "close_pane"
                            })
                    }
                    MenuItem {
                        text: "Move/swap pane…"
                        onTriggered: root.paletteWith("move-pane ")
                    }
                    MenuItem {
                        text: "Terminate terminal"
                        visible: panel.paneData.kind === "terminal"
                        onTriggered: root.send({
                                "action": "terminate_terminal"
                            })
                    }
                    MenuSeparator {
                    }
                    MenuItem {
                        text: "Stage selected"
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.paletteWith("stage")
                    }
                    MenuItem {
                        text: "Unstage selected"
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.paletteWith("unstage")
                    }
                    MenuItem {
                        text: "View diff"
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.paletteWith("diff")
                    }
                    MenuItem {
                        text: "Commit…"
                        visible: panel.paneData.kind === "git"
                        onTriggered: root.paletteWith("commit ")
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
    footer: ToolBar {
        implicitHeight: statusRow.implicitHeight + 2 * Kirigami.Units.smallSpacing
        RowLayout {
            id: statusRow
            anchors.fill: parent
            anchors.margins: Kirigami.Units.smallSpacing
            Label {
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                text: root.frame.status || ""
                elide: Text.ElideRight
                ToolTip.visible: statusHover.hovered
                ToolTip.text: text
                HoverHandler {
                    id: statusHover
                }
            }
            Label {
                text: root.frame.location || ""
                visible: text.length > 0
                Layout.maximumWidth: root.width * 0.4
                elide: Text.ElideMiddle
            }
            ToolButton {
                text: "?"
                onClicked: root.paletteWith("")
                ToolTip.visible: hovered
                ToolTip.text: root.frame.hints || "Keyboard shortcuts"
            }
        }
    }
    Dialog {
        id: editPrompt
        objectName: "editPrompt"
        property var prompt: root.frame.prompt || ({
                "kind": "find",
                "input": "",
                "replacement": "",
                "case_sensitive": false,
                "whole_word": false
            })
        title: prompt.kind === "goto" ? "Go to line" : prompt.kind === "replace" ? "Replace" : "Find"
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
                    placeholderText: editPrompt.prompt.kind === "goto" ? "Line number" : "Find text"
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
                    visible: editPrompt.prompt.kind !== "goto"
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
                    Layout.fillWidth: true
                    text: root.frame.status || ""
                    wrapMode: Text.Wrap
                }
                Flow {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    Button {
                        text: editPrompt.prompt.kind === "goto" ? "Go" : editPrompt.prompt.kind === "replace" ? "Replace next" : "Find next"
                        onClicked: root.send({
                                "action": "submit_prompt"
                            })
                    }
                    Button {
                        text: "Replace all"
                        visible: editPrompt.prompt.kind === "replace"
                        onClicked: root.send({
                                "action": "submit_prompt",
                                "all": true
                            })
                    }
                    Button {
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
        objectName:"commandPalette"
        title: "Slate commands"
        anchors.centerIn: parent
        width: Math.min(root.width - 40, 800)
        height: Math.min(root.height - 40, implicitHeight)
        modal: true
        standardButtons: Dialog.Ok | Dialog.Cancel
        onAccepted: {
            slate.command(commandText.text);
            Qt.callLater(root.focusPane);
        }
        onRejected: Qt.callLater(root.focusPane)
        contentItem: ScrollView {
            id: paletteScroll
            clip: true
            contentWidth: availableWidth
            ColumnLayout {
                width: paletteScroll.availableWidth
                spacing: Kirigami.Units.smallSpacing
                TextField {
                    id: commandText
                    Layout.fillWidth: true
                    placeholderText: "open /path/to/file · save · split-right · terminal"
                    onAccepted: palette.accept()
                }
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    text: "find TEXT · replace TEXT => VALUE · replace-all TEXT => VALUE · goto LINE · indent · outdent · set OPTION VALUE · settings-reload · open PATH · save · save-as PATH · new · close · undo · redo · split-right · split-down · terminal · terminate-terminal · files · git · editor · close-pane · move-pane ID · preset development|minimal|bottom_terminal · layout-save NAME · layout-load NAME · refresh · stage · unstage · diff · commit MESSAGE · quit · discard-quit · discard-document"
                }
            }
        }
    }
    Dialog {
        id: quitDialog
        title: "Unsaved documents"
        anchors.centerIn: parent
        modal: true
        standardButtons: Dialog.Discard | Dialog.Cancel
        width: Math.min(root.width - 40, 480)
        Label {
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
