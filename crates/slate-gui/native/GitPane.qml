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
    // The pane lists its changes, then the commit graph: one selection, owned
    // by the shared core, covers both.
    readonly property int selectedRow: paneData.selected || 0
    readonly property int changesSelected: selectedRow < entries.length ? selectedRow : -1
    readonly property int commitSelected: selectedRow - entries.length
    readonly property bool paneFocused: frame.focus === paneId
    readonly property bool repository: !!frame.git_repository && !frame.git_restricted
    required property string draft
    signal draftEdited(string text)
    signal commitRequested(string message)
    property alias compactComposer: commitDialog
    required property bool committing
    // The graph's state lives with the window, so recreated panes keep it.
    required property bool graphOpen
    required property real graphShare
    signal graphOpenEdited(bool open)
    signal graphShareEdited(real share)
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
    // The kind of change a row shows, for its letter and colour.
    function changeKind(entry) {
        if (entry.group === "Conflicts")
            return "conflict";
        if (entry.untracked)
            return "untracked";
        var code = entry.status.charAt(entry.staged ? 0 : 1);
        return code === "A" ? "added" : code === "D" ? "deleted" : code === "R" || code === "C" ? "renamed" : "modified";
    }
    function changeLetter(entry) {
        var kind = changeKind(entry);
        return kind === "conflict" ? "!" : kind === "untracked" ? "U" : entry.status.charAt(entry.staged ? 0 : 1);
    }
    function absolutePath(relative) {
        var top = (frame.git_root || "").replace(/\/+$/, "");
        return top.length ? top + "/" + relative : relative;
    }
    // "now", "5m", "3h", "2d", "4mo", "1y".
    function ago(seconds) {
        var age = Math.max(0, Date.now() / 1000 - seconds);
        if (age < 60)
            return "now";
        if (age < 3600)
            return Math.floor(age / 60) + "m";
        if (age < 86400)
            return Math.floor(age / 3600) + "h";
        if (age < 2592000)
            return Math.floor(age / 86400) + "d";
        if (age < 31536000)
            return Math.floor(age / 2592000) + "mo";
        return Math.floor(age / 31536000) + "y";
    }
    onCommittingChanged: {
        if (!committing && !frame.git_error && commitDialog.visible)
            commitDialog.close();
    }
    FontMetrics {
        id: metrics
        font: message.font
    }
    // One line per change; tall enough for the compact row actions.
    readonly property int rowHeight: Math.ceil(Math.max(30, metrics.height + 12))
    readonly property int graphRowHeight: Math.ceil(Math.max(24, metrics.height + 8))
    readonly property int sectionHeight: Math.ceil(Math.max(view.compactHeight ? 26 : 30, metrics.height + 10))
    readonly property int iconSize: Math.max(14, Math.min(rowHeight - 10, Math.ceil(metrics.height)))
    // Collapsed change groups, by name.
    property var collapsedGroups: ({})
    function toggleGroup(group) {
        var next = Object.assign({}, collapsedGroups);
        next[group] = !next[group];
        collapsedGroups = next;
    }
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: view.compactHeight ? 6 : 8
        spacing: view.compactHeight ? 4 : 6
        // Branch and sync state, with repository tools at the right. When the
        // pane is too narrow for both, the branch moves below the tools.
        Item {
            id: header
            Layout.fillWidth: true
            readonly property bool stacked: width - tools.width - 8 < 72
            implicitHeight: stacked ? tools.height + 2 + branchRow.height : Math.max(tools.height, branchRow.height)
            RowLayout {
                id: branchRow
                x: 0
                y: header.stacked ? tools.height + 2 : (header.height - height) / 2
                width: header.stacked ? header.width : Math.max(0, header.width - tools.width - 8)
                spacing: 2
                ActionButton {
                    id: branchButton
                    objectName: "gitBranch_" + view.paneId
                    visible: view.repository
                    Layout.minimumWidth: 0
                    Layout.maximumWidth: implicitWidth
                    Layout.fillWidth: true
                    iconName: "git-branch"
                    text: view.frame.git_branch || "No branch"
                    font.bold: true
                    flat: true
                    compact: true
                    tip: "Branch " + text + " · switch or create a branch"
                    enabled: view.commandInfo("git-switch").enabled === true
                    onClicked: branchMenu.popup()
                }
                ActionButton {
                    objectName: "gitSync_" + view.paneId
                    Layout.minimumWidth: implicitWidth
                    visible: view.repository && !!view.frame.git_upstream && (view.frame.git_ahead > 0 || view.frame.git_behind > 0)
                    text: (view.frame.git_behind > 0 ? "↓" + view.frame.git_behind : "") + (view.frame.git_ahead > 0 && view.frame.git_behind > 0 ? " " : "") + (view.frame.git_ahead > 0 ? "↑" + view.frame.git_ahead : "")
                    tip: [view.frame.git_behind > 0 ? view.frame.git_behind + " to pull" : "", view.frame.git_ahead > 0 ? view.frame.git_ahead + " to push" : ""].filter(function (s) {
                        return s.length;
                    }).join(", ") + " · " + (view.frame.git_upstream || "")
                    compact: true
                    flat: true
                    onClicked: syncMenu.popup()
                }
                Label {
                    visible: !view.repository
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    Layout.leftMargin: 4
                    text: "Source control"
                    textFormat: Text.PlainText
                    font.bold: true
                    elide: Text.ElideRight
                }
                Item {
                    Layout.fillWidth: view.repository
                }
            }
            Row {
                id: tools
                anchors.right: parent.right
                y: header.stacked ? 0 : (header.height - height) / 2
                spacing: 0
                ActionButton {
                    objectName: "gitStageAll_" + view.paneId
                    visible: view.repository
                    iconName: "list-add"
                    tip: "Stage all changes, including untracked files"
                    compact: true
                    flat: true
                    enabled: view.commandInfo("stage-all").enabled === true
                    onClicked: view.invokeAction("stage-all")
                }
                ActionButton {
                    objectName: "gitUnstageAll_" + view.paneId
                    visible: view.repository
                    iconName: "list-remove"
                    tip: "Unstage all changes and keep working files"
                    compact: true
                    flat: true
                    enabled: view.commandInfo("unstage-all").enabled === true
                    onClicked: view.invokeAction("unstage-all")
                }
                ActionButton {
                    objectName: "gitRefresh_" + view.paneId
                    iconName: "view-refresh"
                    tip: "Refresh Git changes and history"
                    compact: true
                    flat: true
                    enabled: view.commandInfo("refresh").enabled === true
                    onClicked: view.invokeAction("refresh")
                }
                ActionButton {
                    objectName: "gitMore_" + view.paneId
                    visible: view.repository
                    iconName: "view-more-symbolic"
                    tip: "More Git actions"
                    compact: true
                    flat: true
                    onClicked: moreMenu.popup()
                }
            }
        }
        Basic.ScrollView {
            id: messageScroll
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            Layout.preferredHeight: Math.ceil(Math.max(66, metrics.height * 2 + 20))
            Layout.minimumHeight: Layout.preferredHeight
            Layout.maximumHeight: Layout.preferredHeight
            visible: view.repository && !view.compactHeight
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
            visible: view.repository
            // The primary action keeps its accent while unavailable.
            highlighted: true
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
        // Errors only: progress is the bar below, which never moves the list.
        Label {
            id: noticeLabel
            objectName: "gitNotice_" + view.paneId
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            visible: !!view.frame.git_error && !view.frame.git_restricted
            text: view.frame.git_error || ""
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            maximumLineCount: view.compactHeight ? 1 : 3
            elide: Text.ElideRight
            color: Theme.negativeTextColor
            HoverHandler {
                id: noticeHover
            }
            Hint {
                anchorItem: noticeLabel
                visible: noticeHover.hovered && !!view.frame.git_error
                text: view.frame.git_error || ""
            }
        }
        // A divider that becomes a progress bar while Git works.
        Item {
            objectName: "gitProgress_" + view.paneId
            Layout.fillWidth: true
            implicitHeight: 2
            Rectangle {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                height: 1
                color: Qt.alpha(Theme.textColor, 0.1)
            }
            Rectangle {
                id: progress
                visible: !!view.frame.git_busy && view.visible
                width: parent.width / 4
                height: 2
                radius: 1
                color: Theme.highlightColor
                NumberAnimation on x {
                    running: progress.visible
                    from: -progress.width
                    to: progress.parent.width
                    duration: 1100
                    loops: Animation.Infinite
                }
            }
        }
        Item {
            id: body
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 0
            readonly property bool graphShown: view.repository
            readonly property bool graphExpanded: graphShown && view.graphOpen && !view.compactHeight
            // The changes keep room for a row and a section header; the graph
            // for its header and two commits.
            readonly property real changesMinimum: view.rowHeight + view.sectionHeight + 8
            readonly property real graphMinimum: graphHeader.height + view.graphRowHeight * 2
            readonly property real graphHeight: !graphShown ? 0 : !graphExpanded ? graphHeader.height : Math.max(graphMinimum, Math.min(height - changesMinimum - splitter.height, height * view.graphShare))
            Item {
                id: viewport
                width: parent.width
                height: Math.max(0, parent.height - body.graphHeight - (body.graphShown ? splitter.height : 0))
                clip: true
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
                    // Selection is drawn from the core's index; the view only
                    // scrolls to it (see the Connections below).
                    highlightFollowsCurrentItem: false
                    keyNavigationEnabled: false
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
                    Connections {
                        target: view
                        function onChangesSelectedChanged() {
                            if (view.changesSelected >= 0 && view.changesSelected < changes.count)
                                changes.positionViewAtIndex(view.changesSelected, ListView.Contain);
                        }
                    }
                    section.property: "group"
                    section.delegate: Item {
                        id: sectionHeader
                        required property string section
                        readonly property bool collapsed: !!view.collapsedGroups[section]
                        width: changes.width
                        height: view.sectionHeight
                        MouseArea {
                            anchors.fill: parent
                            onClicked: view.toggleGroup(sectionHeader.section)
                        }
                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 6
                            spacing: 6
                            Label {
                                text: sectionHeader.collapsed ? "▸" : "▾"
                                textFormat: Text.PlainText
                                color: Theme.disabledTextColor
                                Layout.preferredWidth: view.iconSize
                                horizontalAlignment: Text.AlignHCenter
                            }
                            Label {
                                Layout.minimumWidth: 0
                                text: sectionHeader.section
                                textFormat: Text.PlainText
                                font.bold: true
                                elide: Text.ElideRight
                            }
                            Rectangle {
                                implicitWidth: Math.max(implicitHeight, countLabel.implicitWidth + 10)
                                implicitHeight: countLabel.implicitHeight + 2
                                radius: implicitHeight / 2
                                color: Qt.alpha(Theme.textColor, 0.12)
                                Label {
                                    id: countLabel
                                    anchors.centerIn: parent
                                    text: view.groupCount(sectionHeader.section)
                                    textFormat: Text.PlainText
                                    font.pixelSize: Math.max(9, Math.round(metrics.height * 0.62))
                                }
                            }
                            Item {
                                Layout.fillWidth: true
                            }
                            ActionButton {
                                objectName: "gitGroup_" + view.paneId + "_" + sectionHeader.section
                                iconName: sectionHeader.section === "Staged" ? "list-remove" : "list-add"
                                tip: sectionHeader.section === "Staged" ? "Unstage all staged changes" : "Stage all " + sectionHeader.section.toLowerCase() + " changes"
                                compact: true
                                flat: true
                                enabled: view.commandInfo(sectionHeader.section === "Staged" ? "unstage-all" : "stage-group").enabled === true
                                onClicked: view.invokeAction(sectionHeader.section === "Staged" ? "unstage-all" : "stage-group", sectionHeader.section === "Staged" ? "" : sectionHeader.section)
                            }
                        }
                    }
                    delegate: Basic.ItemDelegate {
                        id: entry
                        required property var modelData
                        required property int index
                        objectName: "entry_" + view.paneId + "_" + index
                        readonly property bool collapsed: !!view.collapsedGroups[modelData.group]
                        readonly property string kind: view.changeKind(modelData)
                        readonly property bool strong: highlighted && view.paneFocused
                        readonly property color foreground: strong ? Theme.highlightedTextColor : Theme.textColor
                        // Narrow rows keep the name readable: actions appear only on
                        // hover there, and the rarer ones stay in the context menu.
                        readonly property bool roomy: width >= 240
                        readonly property bool showActions: hovered || actionsHover.hovered || (roomy && (highlighted || activeFocus))
                        readonly property string filename: modelData.path.split("/").pop()
                        readonly property string directory: modelData.path.indexOf("/") < 0 ? "" : modelData.path.substring(0, modelData.path.lastIndexOf("/"))
                        text: modelData.path
                        Accessible.name: modelData.path + ", " + kind + (modelData.staged ? ", staged" : ", working changes")
                        visible: !collapsed
                        width: changes.width
                        height: collapsed ? 0 : view.rowHeight
                        topPadding: 1
                        bottomPadding: 1
                        leftPadding: 6
                        rightPadding: 6
                        highlighted: index === view.changesSelected
                        hoverEnabled: true
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
                            spacing: 6
                            FileIcon {
                                Layout.alignment: Qt.AlignVCenter
                                extent: view.iconSize
                                name: entry.filename
                                tint: entry.foreground
                            }
                            Label {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                Layout.maximumWidth: implicitWidth
                                text: entry.filename
                                textFormat: Text.PlainText
                                elide: Text.ElideMiddle
                                color: entry.strong ? entry.foreground : Theme.statusColor(entry.kind)
                                font.strikeout: entry.kind === "deleted"
                            }
                            Label {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                text: entry.modelData.original_path ? "← " + entry.modelData.original_path : entry.directory
                                textFormat: Text.PlainText
                                elide: Text.ElideMiddle
                                opacity: 0.6
                                color: entry.foreground
                            }
                            Label {
                                Layout.preferredWidth: letterMetrics.advanceWidth("M") + 4
                                horizontalAlignment: Text.AlignHCenter
                                text: view.changeLetter(entry.modelData)
                                textFormat: Text.PlainText
                                color: entry.strong ? entry.foreground : Theme.statusColor(entry.kind)
                                font.bold: true
                                FontMetrics {
                                    id: letterMetrics
                                    font: entry.font
                                }
                            }
                        }
                        background: Rectangle {
                            x: 2
                            width: parent.width - 4
                            height: parent.height
                            radius: 4
                            color: entry.highlighted ? (view.paneFocused ? Theme.highlightColor : Qt.alpha(Theme.highlightColor, 0.28)) : entry.hovered ? Qt.alpha(Theme.textColor, 0.07) : "transparent"
                        }
                        // Row actions appear over the folder text on hover or
                        // selection, so resting rows stay quiet.
                        Rectangle {
                            id: rowActions
                            anchors.right: parent.right
                            anchors.rightMargin: entry.rightPadding + letterMetrics.advanceWidth("M") + 10
                            anchors.verticalCenter: parent.verticalCenter
                            width: actionRow.implicitWidth + 6
                            height: actionRow.implicitHeight
                            color: entry.background.color.a > 0.5 ? entry.background.color : Qt.tint(Theme.backgroundColor, entry.background.color)
                            opacity: entry.showActions ? 1 : 0
                            HoverHandler {
                                id: actionsHover
                            }
                            Row {
                                id: actionRow
                                anchors.right: parent.right
                                spacing: 0
                                ActionButton {
                                    objectName: "gitOpen_" + view.paneId + "_" + entry.index
                                    foregroundColor: entry.foreground
                                    iconName: "document-open"
                                    tip: "Open file"
                                    compact: true
                                    flat: true
                                    visible: entry.roomy && entry.kind !== "deleted"
                                    enabled: view.commandInfo("git-open", entry.index).enabled === true
                                    onClicked: view.invokeAction("git-open", "", entry.index)
                                }
                                ActionButton {
                                    objectName: "gitDiscard_" + view.paneId + "_" + entry.index
                                    foregroundColor: entry.foreground
                                    iconName: "edit-undo"
                                    tip: entry.modelData.untracked ? "Discard: move the untracked file to Trash…" : "Discard working changes…"
                                    compact: true
                                    flat: true
                                    visible: entry.roomy && !entry.modelData.staged && entry.kind !== "conflict"
                                    enabled: view.commandInfo("discard-changes", entry.index).enabled === true
                                    onClicked: view.invokeAction("discard-changes", "", entry.index)
                                }
                                ActionButton {
                                    objectName: "gitDiff_" + view.paneId + "_" + entry.index
                                    foregroundColor: entry.foreground
                                    iconName: "vcs-diff"
                                    tip: "Inspect " + (entry.modelData.staged ? "staged" : "working") + " changes"
                                    compact: true
                                    flat: true
                                    enabled: view.commandInfo("diff", entry.index).enabled === true
                                    onClicked: entry.inspect()
                                }
                                ActionButton {
                                    objectName: "gitStage_" + view.paneId + "_" + entry.index
                                    foregroundColor: entry.foreground
                                    iconName: entry.modelData.staged ? "list-remove" : "list-add"
                                    tip: entry.modelData.staged ? "Unstage file" : "Stage file"
                                    compact: true
                                    flat: true
                                    enabled: view.commandInfo(entry.modelData.staged ? "unstage" : "stage", entry.index).enabled === true
                                    onClicked: view.invokeAction(entry.modelData.staged ? "unstage" : "stage", "", entry.index)
                                }
                            }
                        }
                        Hint {
                            anchorItem: entry
                            visible: entry.hovered
                            text: entry.modelData.path + (entry.modelData.original_path ? "\nRenamed from " + entry.modelData.original_path : "") + (entry.modelData.staged ? "\nStaged changes" : "\nWorking changes")
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
                            ActionMenuItem {
                                caption: "Open file"
                                enabled: view.commandInfo("git-open", entry.index).enabled === true && entry.kind !== "deleted"
                                onTriggered: view.invokeAction("git-open", "", entry.index)
                            }
                            ActionMenuItem {
                                caption: "Inspect changes"
                                enabled: view.commandInfo("diff", entry.index).enabled === true
                                onTriggered: entry.inspect()
                            }
                            ActionMenuItem {
                                caption: entry.modelData.staged ? "Unstage file" : "Stage file"
                                enabled: view.commandInfo(entry.modelData.staged ? "unstage" : "stage", entry.index).enabled === true
                                onTriggered: view.invokeAction(entry.modelData.staged ? "unstage" : "stage", "", entry.index)
                            }
                            ActionMenuItem {
                                caption: "Discard changes…"
                                enabled: view.commandInfo("discard-changes", entry.index).enabled === true
                                onTriggered: view.invokeAction("discard-changes", "", entry.index)
                            }
                            MenuSeparator {}
                            ActionMenuItem {
                                caption: "Copy path"
                                onTriggered: view.bridge.copyText(view.absolutePath(entry.modelData.path))
                            }
                            ActionMenuItem {
                                caption: "Copy relative path"
                                onTriggered: view.bridge.copyText(entry.modelData.path)
                            }
                            ActionMenuItem {
                                caption: "Reveal in file browser"
                                enabled: view.commandInfo("reveal-in-files", entry.index).enabled === true && entry.kind !== "deleted"
                                onTriggered: view.invokeAction("reveal-in-files", "", entry.index)
                            }
                        }
                    }
                }
                // Git runs nothing until the repository is trusted.
                ColumnLayout {
                    anchors.centerIn: parent
                    width: Math.max(0, parent.width - 16)
                    visible: !!view.frame.git_restricted
                    spacing: Theme.largeSpacing
                    Label {
                        Layout.fillWidth: true
                        text: "Git is off in restricted mode: a repository's configuration can run programs."
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        horizontalAlignment: Text.AlignHCenter
                        color: Theme.disabledTextColor
                    }
                    ActionButton {
                        objectName: "gitTrust_" + view.paneId
                        Layout.alignment: Qt.AlignHCenter
                        text: "Trust Folder…"
                        highlighted: true
                        onClicked: view.invokeAction("request-trust")
                    }
                }
                Label {
                    anchors.centerIn: parent
                    width: Math.max(0, parent.width - 16)
                    visible: changes.count === 0 && !view.frame.git_busy && !view.frame.git_restricted
                    text: view.frame.git_repository ? "Working tree clean\nYour changes will appear here." : "Open a folder containing a Git repository."
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    horizontalAlignment: Text.AlignHCenter
                    color: Theme.disabledTextColor
                }
            }
            // The commit graph, below the changes.
            Item {
                id: graphSection
                objectName: "gitGraphSection_" + view.paneId
                visible: body.graphShown
                y: viewport.height + splitter.height
                width: parent.width
                height: body.graphHeight
                Item {
                    id: graphHeader
                    width: parent.width
                    height: view.sectionHeight
                    MouseArea {
                        anchors.fill: parent
                        enabled: !view.compactHeight
                        onClicked: view.graphOpenEdited(!view.graphOpen)
                    }
                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 6
                        anchors.rightMargin: viewport.gutter
                        spacing: 6
                        Label {
                            text: body.graphExpanded ? "▾" : "▸"
                            textFormat: Text.PlainText
                            color: Theme.disabledTextColor
                            Layout.preferredWidth: view.iconSize
                            horizontalAlignment: Text.AlignHCenter
                        }
                        Label {
                            objectName: "gitGraphTitle_" + view.paneId
                            text: "Graph"
                            textFormat: Text.PlainText
                            font.bold: true
                            font.capitalization: Font.AllUppercase
                            font.letterSpacing: 0.5
                            color: Qt.alpha(Theme.textColor, 0.8)
                        }
                        Label {
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            visible: !!view.frame.git_upstream
                            text: "HEAD · " + (view.frame.git_upstream || "")
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            color: Theme.disabledTextColor
                        }
                        Item {
                            Layout.fillWidth: !view.frame.git_upstream
                        }
                    }
                }
                ListView {
                    id: graph
                    objectName: "gitGraph_" + view.paneId
                    visible: body.graphExpanded
                    y: graphHeader.height
                    width: Math.max(0, parent.width - viewport.gutter)
                    height: Math.max(0, parent.height - y)
                    clip: true
                    model: view.bridge.history
                    boundsBehavior: Flickable.StopAtBounds
                    readonly property real laneWidth: Math.ceil(Math.max(12, metrics.height * 0.75))
                    ScrollBar.vertical: Basic.ScrollBar {
                        parent: graphSection
                        x: graph.width + 4
                        y: graph.y
                        width: viewport.gutter - 4
                        height: graph.height
                        policy: ScrollBar.AsNeeded
                        Accessible.name: "Commit graph scroll position"
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
                    Connections {
                        target: view
                        function onCommitSelectedChanged() {
                            if (view.commitSelected >= 0 && view.commitSelected < graph.count)
                                graph.positionViewAtIndex(view.commitSelected, ListView.Contain);
                        }
                    }
                    Hint {
                        id: commitHint
                        property Item target: graph
                        anchorItem: target
                        visible: false
                    }
                    delegate: Basic.ItemDelegate {
                        id: commitRow
                        required property var modelData
                        required property int index
                        objectName: "commit_" + view.paneId + "_" + index
                        readonly property bool strong: highlighted && view.paneFocused
                        readonly property color foreground: strong ? Theme.highlightedTextColor : Theme.textColor
                        width: graph.width
                        height: view.graphRowHeight
                        // Lanes run edge to edge, joining the rows above and below.
                        topPadding: 0
                        bottomPadding: 0
                        leftPadding: 4
                        rightPadding: 6
                        hoverEnabled: true
                        highlighted: index === view.commitSelected
                        text: modelData.subject
                        Accessible.name: "Commit " + modelData.short + ": " + modelData.subject + ", " + modelData.author
                        contentItem: RowLayout {
                            spacing: 6
                            GraphLanes {
                                Layout.fillHeight: true
                                Layout.preferredWidth: Math.min(commitRow.modelData.lanes, 10) * graph.laneWidth
                                clip: true
                                laneWidth: graph.laneWidth
                                segments: commitRow.modelData.segments
                                lane: commitRow.modelData.lane
                                color: commitRow.modelData.color
                                head: commitRow.modelData.head
                                merge: commitRow.modelData.parents.length > 1
                                background: commitRow.highlighted ? (view.paneFocused ? Theme.highlightColor : Qt.tint(Theme.backgroundColor, Qt.alpha(Theme.highlightColor, 0.28))) : Theme.backgroundColor
                            }
                            Label {
                                Layout.fillWidth: true
                                // The subject keeps priority over badges and author.
                                Layout.minimumWidth: Math.min(implicitWidth, graph.width * 0.4)
                                text: commitRow.modelData.subject
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                                color: commitRow.foreground
                                font.bold: commitRow.modelData.head
                            }
                            Repeater {
                                model: commitRow.modelData.refs.slice(0, graph.width > 300 ? 3 : graph.width > 200 ? 1 : 0)
                                delegate: Rectangle {
                                    required property var modelData
                                    readonly property bool current: modelData.kind === "head"
                                    Layout.minimumWidth: Math.min(implicitWidth, 40)
                                    Layout.maximumWidth: Math.max(40, graph.width * 0.3)
                                    implicitWidth: refLabel.implicitWidth + 10
                                    implicitHeight: refLabel.implicitHeight + 2
                                    radius: implicitHeight / 2
                                    color: current ? (commitRow.strong ? Qt.alpha(Theme.highlightedTextColor, 0.25) : Theme.highlightColor) : "transparent"
                                    border.width: current ? 0 : 1
                                    border.color: Qt.alpha(modelData.kind === "tag" ? Theme.positiveTextColor : commitRow.foreground, modelData.kind === "remote" ? 0.3 : 0.5)
                                    Label {
                                        id: refLabel
                                        anchors.centerIn: parent
                                        width: Math.min(implicitWidth, parent.width - 10)
                                        text: (parent.modelData.kind === "tag" ? "◆ " : "") + parent.modelData.name
                                        textFormat: Text.PlainText
                                        elide: Text.ElideMiddle
                                        font.pixelSize: Math.max(9, Math.round(metrics.height * 0.66))
                                        color: parent.current ? Theme.highlightedTextColor : commitRow.foreground
                                        opacity: parent.modelData.kind === "remote" ? 0.75 : 1
                                    }
                                }
                            }
                            Label {
                                visible: graph.width > 380
                                Layout.maximumWidth: graph.width * 0.22
                                text: commitRow.modelData.author
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                                color: commitRow.foreground
                                opacity: 0.6
                            }
                            Label {
                                text: view.ago(commitRow.modelData.time)
                                textFormat: Text.PlainText
                                color: commitRow.foreground
                                opacity: 0.6
                            }
                        }
                        background: Rectangle {
                            x: 2
                            width: parent.width - 4
                            height: parent.height
                            radius: 4
                            color: commitRow.highlighted ? (view.paneFocused ? Theme.highlightColor : Qt.alpha(Theme.highlightColor, 0.28)) : commitRow.hovered ? Qt.alpha(Theme.textColor, 0.07) : "transparent"
                        }
                        onHoveredChanged: {
                            if (hovered) {
                                var when = new Date(modelData.time * 1000);
                                commitHint.target = commitRow;
                                commitHint.text = modelData.short + " · " + modelData.author + " <" + modelData.email + ">\n" + when.toLocaleString(Qt.locale(), Locale.ShortFormat) + "\n\n" + modelData.subject;
                                commitHint.visible = true;
                            } else if (commitHint.target === commitRow) {
                                commitHint.visible = false;
                            }
                        }
                        // One click opens the commit read-only, as in other editors.
                        onClicked: view.invokeAction("git-show", "", view.entries.length + index)
                        MouseArea {
                            anchors.fill: parent
                            acceptedButtons: Qt.RightButton
                            onClicked: {
                                view.send({
                                    "action": "click",
                                    "pane": view.paneId,
                                    "row": view.entries.length + commitRow.index,
                                    "col": 0
                                });
                                commitMenu.popup();
                            }
                        }
                        Menu {
                            id: commitMenu
                            ActionMenuItem {
                                caption: "Open commit"
                                onTriggered: view.invokeAction("git-show", commitRow.modelData.hash)
                            }
                            MenuSeparator {}
                            ActionMenuItem {
                                caption: "Copy commit hash"
                                onTriggered: view.bridge.copyText(commitRow.modelData.hash)
                            }
                            ActionMenuItem {
                                caption: "Copy subject"
                                onTriggered: view.bridge.copyText(commitRow.modelData.subject)
                            }
                        }
                    }
                    footer: Item {
                        width: graph.width
                        height: view.frame.history_more ? moreButton.implicitHeight + 8 : 0
                        ActionButton {
                            id: moreButton
                            objectName: "gitHistoryMore_" + view.paneId
                            visible: !!view.frame.history_more
                            anchors.centerIn: parent
                            text: "Load more commits"
                            compact: true
                            flat: true
                            onClicked: view.invokeAction("git-history-more")
                        }
                    }
                    Label {
                        // On the view, not its (empty) content.
                        parent: graph
                        anchors.centerIn: parent
                        width: Math.max(0, parent.width - 16)
                        visible: graph.count === 0
                        text: view.frame.history_error ? view.frame.history_error : view.frame.git_busy ? "" : "No commits yet"
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        horizontalAlignment: Text.AlignHCenter
                        color: view.frame.history_error ? Theme.negativeTextColor : Theme.disabledTextColor
                    }
                }
            }
            // Drag to divide the pane between the changes and the graph, like
            // the dividers between panes. Double-click restores the default.
            Rectangle {
                id: splitter
                objectName: "gitGraphSplitter_" + view.paneId
                visible: body.graphShown
                y: viewport.height
                width: parent.width
                height: 6
                radius: 2
                color: !body.graphExpanded ? "transparent" : resize.pressed || resize.containsMouse ? Theme.highlightColor : Qt.alpha(Theme.textColor, 0.18)
                Rectangle {
                    // At rest, collapsed: a plain divider.
                    visible: !body.graphExpanded
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width
                    height: 1
                    color: Qt.alpha(Theme.textColor, 0.1)
                }
                MouseArea {
                    id: resize
                    anchors.fill: parent
                    // A taller target than the bar itself.
                    anchors.topMargin: -3
                    anchors.bottomMargin: -3
                    enabled: body.graphExpanded
                    hoverEnabled: true
                    cursorShape: Qt.SplitVCursor
                    Accessible.role: Accessible.Separator
                    Accessible.name: "Resize the commit graph"
                    property real grab: 0
                    onPressed: function (mouse) {
                        grab = mouse.y;
                    }
                    onPositionChanged: function (mouse) {
                        if (!pressed || body.height <= 0)
                            return;
                        // The bar follows the pointer; each side keeps its minimum.
                        var top = mapToItem(body, 0, mouse.y - grab).y - resize.anchors.topMargin;
                        var graph = body.height - top - splitter.height;
                        graph = Math.max(body.graphMinimum, Math.min(body.height - body.changesMinimum - splitter.height, graph));
                        view.graphShareEdited(graph / body.height);
                    }
                    onDoubleClicked: view.graphShareEdited(0.4)
                }
            }
        }
    }
    // Branches to switch to, and branch creation.
    Menu {
        id: branchMenu
        objectName: "gitBranchMenu_" + view.paneId
        Instantiator {
            model: (view.frame.git_branches || []).filter(function (name) {
                return name !== view.frame.git_branch;
            }).slice(0, 30)
            delegate: ActionMenuItem {
                required property var modelData
                caption: modelData
                onTriggered: view.invokeAction("git-switch", modelData)
            }
            onObjectAdded: function (index, object) {
                branchMenu.insertItem(index, object);
            }
            onObjectRemoved: function (index, object) {
                branchMenu.removeItem(object);
            }
        }
        MenuSeparator {
            visible: (view.frame.git_branches || []).length > 1
            height: visible ? implicitHeight : 0
        }
        ActionMenuItem {
            caption: "Switch to branch…"
            onTriggered: view.invokeAction("git-switch")
        }
        ActionMenuItem {
            caption: "Create branch…"
            onTriggered: view.invokeAction("git-branch")
        }
    }
    Menu {
        id: syncMenu
        ActionMenuItem {
            caption: "Pull" + (view.frame.git_behind > 0 ? " (" + view.frame.git_behind + ")" : "")
            enabled: view.commandInfo("git-pull").enabled === true
            onTriggered: view.invokeAction("git-pull")
        }
        ActionMenuItem {
            caption: "Push" + (view.frame.git_ahead > 0 ? " (" + view.frame.git_ahead + ")" : "")
            enabled: view.commandInfo("git-push").enabled === true
            onTriggered: view.invokeAction("git-push")
        }
        ActionMenuItem {
            caption: "Fetch"
            enabled: view.commandInfo("git-fetch").enabled === true
            onTriggered: view.invokeAction("git-fetch")
        }
    }
    Menu {
        id: moreMenu
        objectName: "gitMoreMenu_" + view.paneId
        ActionMenuItem {
            caption: "Fetch"
            enabled: view.commandInfo("git-fetch").enabled === true
            onTriggered: view.invokeAction("git-fetch")
        }
        ActionMenuItem {
            caption: "Pull"
            enabled: view.commandInfo("git-pull").enabled === true
            onTriggered: view.invokeAction("git-pull")
        }
        ActionMenuItem {
            caption: "Push"
            enabled: view.commandInfo("git-push").enabled === true
            onTriggered: view.invokeAction("git-push")
        }
        MenuSeparator {}
        ActionMenuItem {
            caption: "Switch branch…"
            enabled: view.commandInfo("git-switch").enabled === true
            onTriggered: view.invokeAction("git-switch")
        }
        ActionMenuItem {
            caption: "Create branch…"
            enabled: view.commandInfo("git-branch").enabled === true
            onTriggered: view.invokeAction("git-branch")
        }
        MenuSeparator {}
        ActionMenuItem {
            caption: body.graphExpanded ? "Hide graph" : "Show graph"
            enabled: !view.compactHeight
            onTriggered: view.graphOpenEdited(!view.graphOpen)
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
                    text: view.frame.git_error || ""
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.negativeTextColor
                }
            }
        }
        footer: RowLayout {
            spacing: 6
            Item {
                Layout.fillWidth: true
            }
            ActionButton {
                Layout.bottomMargin: 8
                text: "Cancel"
                enabled: !view.committing
                onClicked: commitDialog.close()
            }
            ActionButton {
                objectName: "gitDialogCommit_" + view.paneId
                Layout.bottomMargin: 8
                Layout.rightMargin: 8
                text: view.committing ? "Committing…" : "Commit staged"
                highlighted: true
                enabled: view.commandInfo("commit").enabled === true && view.draft.trim().length > 0
                onClicked: view.commit()
            }
        }
    }
}
