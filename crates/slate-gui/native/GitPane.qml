import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Basic as Basic
import Slate.Native
import QtQuick.Layouts

FocusScope {
    id: view
    required property var bridge
    required property var frame
    required property var paneData
    required property int paneId
    required property var commandInfoFor
    signal actionRequested(var action)
    readonly property var entries: frame.git || []
    readonly property int stagedCount: entries.filter(function (e) {
        return e.staged;
    }).length
    readonly property int unstagedCount: entries.length - stagedCount
    readonly property bool compactHeight: height < 260 + metrics.height * 3
    required property string draft
    signal draftEdited(string text)
    signal commitRequested(string message)
    property alias compactComposer: commitDialog
    required property bool committing
    function focusContent() {
        // A mouse click may already have focused a message field or row action.
        // Synchronizing Rust focus must preserve that specific Qt control.
        if (!activeFocus && !commitDialog.visible)
            changes.forceActiveFocus();
    }
    function groupCount(group) {
        return entries.filter(function (e) {
            return e.group === group;
        }).length;
    }
    function send(action) {
        if (action.action !== "focus" && frame.focus !== paneId)
            actionRequested({
                "action": "focus",
                "pane": paneId
            });
        actionRequested(action);
    }
    function commandInfo(id, row) {
        return commandInfoFor(id, row);
    }
    function invokeAction(id, argument, row) {
        if (row !== undefined && row >= 0)
            send({"action": "click", "pane": paneId, "row": row, "col": 0});
        send({"action": "invoke_action", "id": id, "argument": argument || ""});
    }
    function commit() {
        if (!commandInfo("commit").enabled || !draft.trim().length)
            return;
        if (frame.focus !== paneId)
            send({
                "action": "focus",
                "pane": paneId
            });
        commitRequested(draft);
    }
    onCommittingChanged: {
        if (!committing && !frame.git_error && commitDialog.visible)
            commitDialog.close();
    }
    FontMetrics {
        id: metrics
        font: message.font
    }
    readonly property int rowHeight: Math.ceil(Math.max(54, metrics.height * 2 + 16))
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: view.compactHeight ? 6 : 8
        spacing: view.compactHeight ? 4 : 8
        RowLayout {
            visible: !view.compactHeight || !view.frame.git_repository
            Layout.fillWidth: true
            spacing: 6
            Image {
                source: slate.hasIcon("git-branch") ? "image://icon/git-branch?" + Theme.textColor : ""
                sourceSize: Qt.size(18, 18)
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                visible: view.frame.git_repository && status === Image.Ready
            }
            Label {
                id: branchLabel
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                text: view.frame.git_repository ? view.frame.git_branch : "Source control"
                textFormat: Text.PlainText
                font.bold: true
                elide: Text.ElideMiddle
                HoverHandler {
                    id: branchHover
                }
                Hint {
                    anchorItem: branchLabel
                    visible: branchHover.hovered
                    text: branchLabel.text
                }
            }
            ActionButton {
                objectName: (view.compactHeight && view.frame.git_repository ? "hiddenGitRefresh_" : "gitRefresh_") + view.paneId
                iconName: "view-refresh"
                tip: "Refresh Git changes"
                flat: true
                enabled: view.commandInfo("refresh").enabled === true
                onClicked: view.invokeAction("refresh")
            }
        }
        Basic.ScrollView {
            id: messageScroll
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.preferredHeight: Math.ceil(Math.max(66, metrics.height * 2 + 20))
            Layout.minimumHeight: Layout.preferredHeight
            Layout.maximumHeight: Layout.preferredHeight
            visible: view.frame.git_repository && !view.compactHeight
            clip: true
            contentWidth: availableWidth
            background: Rectangle {
                radius: 4
                color: Theme.alternateBackgroundColor
                border.color: message.activeFocus ? Theme.highlightColor : Qt.alpha(Theme.textColor, 0.25)
                border.width: message.activeFocus ? 2 : 1
            }
            Basic.TextArea {
                id: message
                objectName: "gitMessage_" + view.paneId
                width: messageScroll.availableWidth
                enabled: !view.committing
                text: view.draft
                onTextChanged: if (activeFocus)
                    view.draftEdited(text)
                onActiveFocusChanged: if (activeFocus && view.visible && view.frame.focus !== view.paneId)
                    view.send({
                        "action": "focus",
                        "pane": view.paneId
                    })
                placeholderText: "Commit message"
                Accessible.description: "Press Ctrl+Enter to commit staged changes"
                wrapMode: TextEdit.Wrap
                selectByMouse: true
                padding: 8
                color: Theme.textColor
                selectionColor: Theme.highlightColor
                selectedTextColor: Theme.highlightedTextColor
                background: null
                Keys.onPressed: function (event) {
                    if (view.frame.focus !== view.paneId)
                        view.send({
                            "action": "focus",
                            "pane": view.paneId
                        });
                    if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && (event.modifiers & Qt.ControlModifier)) {
                        view.commit();
                        event.accepted = true;
                    }
                }
            }
        }
        ActionButton {
            id: commitButton
            objectName: "gitCommit_" + view.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            visible: view.frame.git_repository
            highlighted: enabled
            text: view.committing ? "Committing…" : (view.compactHeight ? "Commit…" : "Commit") + (view.stagedCount ? " (" + view.stagedCount + ")" : "")
            tip: !view.stagedCount ? "Stage changes before committing" : !view.draft.trim().length ? "Enter a commit message" : "Commit staged changes (Ctrl+Enter)"
            enabled: view.commandInfo("commit").enabled === true && (view.compactHeight || view.draft.trim().length > 0)
            onClicked: {
                if (view.compactHeight) {
                    commitDialog.open();
                    dialogMessage.forceActiveFocus();
                } else
                    view.commit();
            }
        }
        Flow {
            objectName: "gitBulkActions_" + view.paneId
            Layout.fillWidth: true
            Layout.preferredHeight: implicitHeight
            spacing: 6
            visible: view.frame.git_repository
            ActionButton {
                objectName: "gitStageAll_" + view.paneId
                text: "Stage all"
                compact: true
                tip: "Stage all changes, including untracked files"
                enabled: view.commandInfo("stage-all").enabled === true
                onClicked: view.invokeAction("stage-all")
            }
            ActionButton {
                objectName: "gitUnstageAll_" + view.paneId
                text: "Unstage all"
                compact: true
                tip: "Unstage all changes and keep working files"
                enabled: view.commandInfo("unstage-all").enabled === true
                onClicked: view.invokeAction("unstage-all")
            }
            ActionButton {
                objectName: (view.compactHeight ? "gitRefresh_" : "hiddenGitRefresh_") + view.paneId
                visible: view.compactHeight
                iconName: "view-refresh"
                tip: "Refresh Git changes"
                compact: true
                flat: true
                enabled: view.commandInfo("refresh").enabled === true
                onClicked: view.invokeAction("refresh")
            }
        }
        Label {
            id: noticeLabel
            objectName: "gitNotice_" + view.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            visible: view.frame.git_busy || !!view.frame.git_error
            text: view.frame.git_error || (view.committing ? "Creating commit…" : "Updating changes…")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            maximumLineCount: view.compactHeight ? 1 : 3
            elide: Text.ElideRight
            color: view.frame.git_error ? Theme.negativeTextColor : Theme.disabledTextColor
            HoverHandler {
                id: noticeHover
            }
            Hint {
                anchorItem: noticeLabel
                visible: noticeHover.hovered && !!view.frame.git_error
                text: view.frame.git_error
            }
        }
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 1
            color: Qt.alpha(Theme.textColor, 0.2)
        }
        Item {
            id: viewport
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 0
            // The gutter belongs to the viewport, never to a file row. Keeping
            // it reserved also prevents controls jumping as the bar appears.
            readonly property real gutter: Math.max(12, gitScroll.implicitWidth) + 4
            ListView {
                id: changes
                objectName: (view.visible ? "browser_" : "hiddenGitBrowser_") + view.paneId
                width: Math.max(0, viewport.width - viewport.gutter)
                height: viewport.height
                clip: true
                model: view.bridge.git
                currentIndex: view.paneData.selected || 0
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: Basic.ScrollBar {
                    id: gitScroll
                    objectName: "gitScroll_" + view.paneId
                    parent: viewport
                    x: changes.width + 4
                    y: 0
                    width: viewport.gutter - 4
                    height: viewport.height
                    policy: ScrollBar.AlwaysOn
                    visible: size < 1
                    Accessible.name: "Git changes scroll position"
                }
                Keys.onPressed: function (event) {
                    if (view.frame.focus !== view.paneId)
                        view.send({
                            "action": "focus",
                            "pane": view.paneId
                        });
                    view.bridge.key(event.key, event.text, event.modifiers);
                    event.accepted = true;
                }
                section.property: "group"
                section.delegate: RowLayout {
                    required property string section
                    width: changes.width
                    height: Math.ceil(Math.max(view.compactHeight ? 28 : 38, metrics.height + (view.compactHeight ? 10 : 16)))
                    spacing: 4
                    Label {
                        Layout.fillWidth: true
                        Layout.minimumWidth: 0
                        text: section + " (" + view.groupCount(section) + ")"
                        textFormat: Text.PlainText
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    ActionButton {
                        objectName: "gitGroup_" + view.paneId + "_" + section
                        iconName: section === "Staged" ? "list-remove" : "list-add"
                        tip: section === "Staged" ? "Unstage all staged changes" : "Stage all " + section.toLowerCase() + " changes"
                        compact: true
                        flat: true
                        enabled: view.commandInfo(section === "Staged" ? "unstage-all" : "stage-group").enabled === true
                        onClicked: view.invokeAction(section === "Staged" ? "unstage-all" : "stage-group", section === "Staged" ? "" : section)
                    }
                }
                delegate: Basic.ItemDelegate {
                    id: entry
                    required property var modelData
                    required property int index
                    objectName: "entry_" + view.paneId + "_" + index
                    text: modelData.path
                    Accessible.name: modelData.path + (modelData.staged ? ", staged changes" : ", working changes")
                    width: changes.width
                    height: view.rowHeight
                    padding: 4
                    highlighted: index === changes.currentIndex
                    hoverEnabled: true
                    readonly property string filename: modelData.path.split("/").pop()
                    readonly property string directory: modelData.path.indexOf("/") < 0 ? "Workspace" : modelData.path.substring(0, modelData.path.lastIndexOf("/"))
                    function selectEntry() {
                        view.send({
                            "action": "click",
                            "pane": view.paneId,
                            "row": index,
                            "col": 0
                        });
                        changes.forceActiveFocus();
                    }
                    function inspect() {
                        selectEntry();
                        view.invokeAction("diff", "", index);
                    }
                    contentItem: RowLayout {
                        spacing: 4
                        ColumnLayout {
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            spacing: 0
                            Label {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                text: entry.filename
                                textFormat: Text.PlainText
                                elide: Text.ElideMiddle
                                color: entry.highlighted ? Theme.highlightedTextColor : Theme.textColor
                            }
                            Label {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                text: entry.modelData.original_path ? entry.modelData.original_path + " → " + entry.directory : entry.directory
                                textFormat: Text.PlainText
                                elide: Text.ElideMiddle
                                opacity: 0.75
                                color: entry.highlighted ? Theme.highlightedTextColor : Theme.textColor
                            }
                        }
                        Label {
                            text: entry.modelData.untracked ? "U" : entry.modelData.group === "Conflicts" ? "!" : entry.modelData.status.charAt(entry.modelData.staged ? 0 : 1)
                            textFormat: Text.PlainText
                            color: entry.highlighted ? Theme.highlightedTextColor : Theme.neutralTextColor
                            font.bold: true
                        }
                        ActionButton {
                            objectName: "gitDiff_" + view.paneId + "_" + entry.index
                            foregroundColor: entry.highlighted ? Theme.highlightedTextColor : Theme.textColor
                            iconName: "vcs-diff"
                            tip: "Inspect " + (entry.modelData.staged ? "staged" : "working") + " changes"
                            compact: true
                            flat: true
                            enabled: view.commandInfo("diff", entry.index).enabled === true
                            onClicked: entry.inspect()
                        }
                        ActionButton {
                            objectName: "gitStage_" + view.paneId + "_" + entry.index
                            foregroundColor: entry.highlighted ? Theme.highlightedTextColor : Theme.textColor
                            iconName: entry.modelData.staged ? "list-remove" : "list-add"
                            tip: entry.modelData.staged ? "Unstage file" : "Stage file"
                            compact: true
                            flat: true
                            enabled: view.commandInfo(entry.modelData.staged ? "unstage" : "stage", entry.index).enabled === true
                            onClicked: view.invokeAction(entry.modelData.staged ? "unstage" : "stage", "", entry.index)
                        }
                    }
                    background: Rectangle {
                        radius: 3
                        color: entry.highlighted ? Theme.highlightColor : entry.hovered ? Theme.alternateBackgroundColor : "transparent"
                    }
                    Hint {
                        anchorItem: entry
                        visible: entry.hovered
                        text: entry.modelData.path + (entry.modelData.staged ? "\nStaged changes" : "\nWorking changes")
                    }
                    onClicked: selectEntry()
                    onDoubleClicked: inspect()
                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.RightButton
                        onClicked: {
                            entry.selectEntry();
                            contextMenu.popup();
                        }
                    }
                    Menu {
                        id: contextMenu
                        MenuItem {
                            text: "Inspect changes"
                            enabled: view.commandInfo("diff", entry.index).enabled === true
                            onTriggered: entry.inspect()
                        }
                        MenuItem {
                            text: entry.modelData.staged ? "Unstage file" : "Stage file"
                            enabled: view.commandInfo(entry.modelData.staged ? "unstage" : "stage", entry.index).enabled === true
                            onTriggered: view.invokeAction(entry.modelData.staged ? "unstage" : "stage", "", entry.index)
                        }
                    }
                }
            }
            Label {
                anchors.centerIn: parent
                width: Math.max(0, parent.width - 16)
                visible: changes.count === 0 && !view.frame.git_busy
                text: view.frame.git_repository ? "Working tree clean\nYour changes will appear here." : "Open a folder containing a Git repository."
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                horizontalAlignment: Text.AlignHCenter
                color: Theme.disabledTextColor
            }
        }
    }
    Dialog {
        id: commitDialog
        objectName: "gitCommitDialog_" + view.paneId
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(560, parent.width - 40)
        height: Math.min(parent.height - 40, implicitHeight)
        title: "Commit staged changes"
        modal: true
        contentItem: ScrollView {
            id: dialogScroll
            clip: true
            contentWidth: availableWidth
            implicitHeight: 180
            ColumnLayout {
                width: dialogScroll.availableWidth
                Basic.TextArea {
                    id: dialogMessage
                    objectName: "gitDialogMessage_" + view.paneId
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    text: view.draft
                    onTextChanged: if (activeFocus)
                        view.draftEdited(text)
                    onActiveFocusChanged: if (activeFocus && view.visible && view.frame.focus !== view.paneId)
                        view.send({
                            "action": "focus",
                            "pane": view.paneId
                        })
                    enabled: !view.committing
                    placeholderText: "Commit message (Ctrl+Enter)"
                    wrapMode: TextEdit.Wrap
                    selectByMouse: true
                    Keys.onPressed: function (event) {
                        if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && (event.modifiers & Qt.ControlModifier)) {
                            view.commit();
                            event.accepted = true;
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    visible: !!view.frame.git_error
                    text: view.frame.git_error
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.negativeTextColor
                }
            }
        }
        footer: Flow {
            padding: 8
            spacing: 6
            ActionButton {
                text: "Cancel"
                enabled: !view.committing
                onClicked: commitDialog.close()
            }
            ActionButton {
                objectName: "gitDialogCommit_" + view.paneId
                text: view.committing ? "Committing…" : "Commit staged"
                highlighted: enabled
                enabled: view.commandInfo("commit").enabled === true && view.draft.trim().length > 0
                onClicked: view.commit()
            }
        }
    }
}
