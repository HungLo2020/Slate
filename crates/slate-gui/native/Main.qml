import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import Slate.Native

Kirigami.ApplicationWindow {
    id: root
    objectName: "slateWindow"
    width: 1360; height: 820
    minimumWidth: 480; minimumHeight: 320
    visible: true
    title: "Slate"
    property var frame: slate.frame
    property int priorFocus: -1
    function send(action) { slate.send(action); slate.refresh(); if(!frame.prompt && !palette.visible && !quitDialog.visible) Qt.callLater(root.focusPane) }
    function pane(id) { var items = frame.panes || []; for (var i=0;i<items.length;i++) if(items[i].id===id) return items[i]; return ({id:id,kind:"files",rect:{x:0,y:0,width:0,height:0},tabs:[]}) }
    function handle(id) { var items=frame.handles||[];for(var i=0;i<items.length;i++)if(items[i].id===id)return items[i];return ({id:id,axis:"horizontal",rect:{x:0,y:0,width:0,height:0},parent:{x:0,y:0,width:1,height:1}}) }
    function focusPane() { for(var i=0;i<panes.count;i++){var item=panes.itemAt(i);if(item&&item.paneId===frame.focus)item.focusContent()} }
    function paletteWith(text) { commandText.text=text || ""; palette.open(); commandText.forceActiveFocus(); commandText.cursorPosition=commandText.text.length }
    onClosing: function(close) {close.accepted=false;slate.exit();if(slate.frame.dirty)quitDialog.open()}
    Connections {
        target: slate
        function onFrameChanged() { if(root.frame.prompt && !editPrompt.visible){editPrompt.open();promptInput.forceActiveFocus()} else if(!root.frame.prompt && editPrompt.visible){editPrompt.close();Qt.callLater(root.focusPane)} if(root.priorFocus!==root.frame.focus){root.priorFocus=root.frame.focus;Qt.callLater(root.focusPane)} }
    }
    Shortcut { sequence: "F1"; onActivated: root.paletteWith("") }
    Shortcut { sequence: "Ctrl+Shift+P"; onActivated: root.paletteWith("") }
    header: ToolBar {
        implicitHeight:40
        Flickable {
            anchors.fill:parent
            contentWidth:toolbarRow.implicitWidth
            contentHeight:height
            clip:true
            flickableDirection:Flickable.HorizontalFlick
            RowLayout {
            id:toolbarRow
            height:parent.height
            ToolButton {text:"Open…";onClicked:root.paletteWith("open ")}
            ToolButton {text:"New";onClicked:root.send({action:"new"})}
            ToolButton {text:"Save";onClicked:root.send({action:"save"})}
            ToolButton {text:"Save as…";onClicked:root.paletteWith("save-as ")}
            ToolButton {text:"Find…";onClicked:root.send({action:"prompt",kind:"find"})}
            ToolButton {text:"Replace…";onClicked:root.send({action:"prompt",kind:"replace"})}
            ToolButton {text:"Go to…";onClicked:root.send({action:"prompt",kind:"goto"})}
            ToolButton {text:"Split ▸";onClicked:root.send({action:"split",axis:"horizontal"})}
            ToolButton {text:"Split ▾";onClicked:root.send({action:"split",axis:"vertical"})}
            ToolButton {text:"Terminal +";onClicked:root.send({action:"new_terminal"})}
            ToolButton {text:"Layout";onClicked:layoutMenu.open()}
            Item {width:8}
            ToolButton {text:"Commands";onClicked:root.paletteWith("")}
            ToolButton {text:"Quit";onClicked:root.close()}
            }
        }
        Menu {
            id:layoutMenu
            MenuItem {text:"Three panes";onTriggered:root.send({action:"preset",name:"development"})}
            MenuItem {text:"Terminal below";onTriggered:root.send({action:"preset",name:"bottom_terminal"})}
            MenuItem {text:"Editor only";onTriggered:root.send({action:"preset",name:"minimal"})}
            MenuSeparator {}
            MenuItem {text:"Editor settings…";onTriggered:root.paletteWith("set ")}
            MenuItem {text:"Reload settings";onTriggered:root.send({action:"reload_settings"})}
            MenuSeparator {}
            MenuItem {text:"Save named layout…";onTriggered:root.paletteWith("layout-save ")}
            MenuItem {text:"Load named layout…";onTriggered:root.paletteWith("layout-load ")}
        }
    }
    Item {
        id:workspace
        anchors.fill:parent
        onWidthChanged:slate.viewport(width,height)
        onHeightChanged:slate.viewport(width,height)
        Component.onCompleted: {slate.viewport(width,height);Qt.callLater(root.focusPane)}
        Repeater {
            id:panes
            model:slate.paneIds
            delegate:Rectangle {
                id:panel
                required property var modelData
                property int paneId:modelData
                property var paneData:root.pane(paneId)
                x:paneData.rect.x;y:paneData.rect.y;width:paneData.rect.width;height:paneData.rect.height
                color:Kirigami.Theme.backgroundColor
                border.color:root.frame.focus===paneId?Kirigami.Theme.highlightColor:Kirigami.Theme.disabledTextColor
                border.width:root.frame.focus===paneId?2:1
                clip:true
                function focusContent(){if(paneData.kind==="editor"||paneData.kind==="terminal")grid.forceActiveFocus();else browser.forceActiveFocus()}
                Flickable {
                    id:tabs
                    height:30;width:parent.width
                    contentWidth:tabRow.width
                    contentHeight:height
                    flickableDirection:Flickable.HorizontalFlick
                    clip:true
                    Row {
                    id:tabRow
                    Repeater {
                        model:panel.paneData.tabs
                        delegate:Button {
                            required property var modelData
                            required property int index
                            text:modelData.title
                            height:30;implicitWidth:Math.min(180,Math.max(65,implicitContentWidth+24))
                            highlighted:modelData.active
                            font.bold:modelData.active
                            ToolTip.visible:hovered
                            ToolTip.text:modelData.title
                            onClicked:{root.send({action:"switch_tab",pane:panel.paneId,index:index});panel.focusContent()}
                        }
                    }
                    }
                }
                CellView {
                    id:grid
                    objectName:"cells_"+panel.paneId
                    x:1;y:31;width:parent.width-2;height:parent.height-32
                    visible:panel.paneData.kind==="editor"||panel.paneData.kind==="terminal"
                    pane:panel.paneData
                }
                ListView {
                    id:browser
                    objectName:"browser_"+panel.paneId
                    x:1;y:31;width:parent.width-2;height:parent.height-32
                    visible:panel.paneData.kind==="files"||panel.paneData.kind==="git"
                    clip:true
                    model:panel.paneData.kind==="git"?slate.git:slate.files
                    currentIndex:panel.paneData.selected||0
                    ScrollBar.vertical:ScrollBar {}
                    Keys.onPressed:function(event){slate.key(event.key,event.text,event.modifiers);event.accepted=true}
                    delegate:ItemDelegate {
                        required property var modelData
                        required property int index
                        height:28;width:browser.width
                        text:panel.paneData.kind==="git"?modelData.status+"  "+modelData.path:(modelData.directory?"▸  ":"   ")+modelData.name
                        highlighted:index===browser.currentIndex
                        onClicked:{root.send({action:"click",pane:panel.paneId,row:index,col:0});browser.forceActiveFocus()}
                        onDoubleClicked:{root.send({action:"focus",pane:panel.paneId});if(panel.paneData.kind==="git")root.send({action:"git_diff",path:modelData.path});else root.send({action:"open",path:modelData.path})}
                    }
                }
                MouseArea {anchors.fill:parent;acceptedButtons:Qt.RightButton;enabled:panel.paneData.kind!=="terminal";onClicked:{root.send({action:"focus",pane:panel.paneId});paneMenu.open()}}
                Menu {
                    id:paneMenu
                    MenuItem {text:"Copy selection";onTriggered:slate.copyClipboard()}
                    MenuItem {text:"Paste";onTriggered:slate.pasteClipboard()}
                    MenuSeparator {}
                    MenuItem {text:"Files view";onTriggered:root.send({action:"add_view",kind:"files"})}
                    MenuItem {text:"Git view";onTriggered:root.send({action:"add_view",kind:"git"})}
                    MenuItem {text:"Editor view";onTriggered:root.send({action:"add_view",kind:"editor"})}
                    MenuItem {text:"Terminal view";onTriggered:root.send({action:"new_terminal"})}
                    MenuSeparator {}
                    MenuItem {text:"Close pane";onTriggered:root.send({action:"close_pane"})}
                    MenuItem {text:"Move/swap pane…";onTriggered:root.paletteWith("move-pane ")}
                    MenuItem {text:"Terminate terminal";onTriggered:root.send({action:"terminate_terminal"})}
                    MenuSeparator {}
                    MenuItem {text:"Stage selected";onTriggered:root.paletteWith("stage")}
                    MenuItem {text:"Unstage selected";onTriggered:root.paletteWith("unstage")}
                    MenuItem {text:"View diff";onTriggered:root.paletteWith("diff")}
                    MenuItem {text:"Commit…";onTriggered:root.paletteWith("commit ")}
                }
                Label {anchors.right:parent.right;anchors.bottom:parent.bottom;text:"#"+panel.paneId;font.pixelSize:10;color:Kirigami.Theme.disabledTextColor}
            }
        }
        Repeater {
            model:slate.handleIds
            delegate:Rectangle {
                required property var modelData
                property var handleData:root.handle(modelData)
                x:handleData.rect.x;y:handleData.rect.y;width:handleData.rect.width;height:handleData.rect.height
                color:drag.containsMouse?Kirigami.Theme.highlightColor:Kirigami.Theme.disabledTextColor
                MouseArea {
                    id:drag;anchors.fill:parent;hoverEnabled:true
                    cursorShape:parent.handleData.axis==="horizontal"?Qt.SplitHCursor:Qt.SplitVCursor
                    onPositionChanged:function(mouse){if(pressed){var p=mapToItem(workspace,mouse.x,mouse.y);var h=parent.handleData;var ratio=h.axis==="horizontal"?(p.x-h.parent.x)/h.parent.width:(p.y-h.parent.y)/h.parent.height;root.send({action:"resize_split",id:h.id,ratio:ratio})}}
                }
            }
        }
    }
    footer:ToolBar {Label {anchors.fill:parent;anchors.margins:6;text:(root.frame.status||"")+"    · "+(root.frame.location||"")+"    · "+(root.frame.hints||"");elide:Text.ElideRight}}
    Dialog {
        id:editPrompt
        objectName:"editPrompt"
        property var prompt:root.frame.prompt||({kind:"find",input:"",replacement:"",case_sensitive:false,whole_word:false})
        title:prompt.kind==="goto"?"Go to line":prompt.kind==="replace"?"Replace":"Find"
        anchors.centerIn:parent
        width:Math.min(root.width-40,560)
        modal:true
        closePolicy:Popup.CloseOnEscape
        onRejected:root.send({action:"dismiss_prompt"})
        function update(){root.send({action:"update_prompt",input:promptInput.text,replacement:replacementInput.text,case_sensitive:matchCase.checked,whole_word:wholeWord.checked})}
        ColumnLayout {
            anchors.fill:parent
            TextField {id:promptInput;objectName:"searchInput";Layout.fillWidth:true;text:editPrompt.prompt.input;placeholderText:editPrompt.prompt.kind==="goto"?"Line number":"Find text";onTextEdited:editPrompt.update();onAccepted:root.send({action:"submit_prompt"})}
            TextField {id:replacementInput;objectName:"replacementInput";Layout.fillWidth:true;visible:editPrompt.prompt.kind==="replace";text:editPrompt.prompt.replacement;placeholderText:"Replacement text";onTextEdited:editPrompt.update();onAccepted:root.send({action:"submit_prompt"})}
            RowLayout {
                visible:editPrompt.prompt.kind!=="goto"
                CheckBox {id:matchCase;text:"Match case";checked:editPrompt.prompt.case_sensitive;onClicked:editPrompt.update()}
                CheckBox {id:wholeWord;text:"Whole word";checked:editPrompt.prompt.whole_word;onClicked:editPrompt.update()}
            }
            Label {Layout.fillWidth:true;text:root.frame.status||"";wrapMode:Text.Wrap}
            RowLayout {
                Button {text:editPrompt.prompt.kind==="goto"?"Go":editPrompt.prompt.kind==="replace"?"Replace next":"Find next";onClicked:root.send({action:"submit_prompt"})}
                Button {text:"Replace all";visible:editPrompt.prompt.kind==="replace";onClicked:root.send({action:"submit_prompt",all:true})}
                Button {text:"Close";onClicked:root.send({action:"dismiss_prompt"})}
            }
        }
    }
    Dialog {
        id:palette
        title:"Slate commands"
        anchors.centerIn:parent
        width:Math.min(root.width-40,800)
        modal:true
        standardButtons:Dialog.Ok|Dialog.Cancel
        onAccepted:{slate.command(commandText.text);Qt.callLater(root.focusPane)}
        onRejected:Qt.callLater(root.focusPane)
        ColumnLayout {
            anchors.fill:parent
            TextField {id:commandText;Layout.fillWidth:true;placeholderText:"open /path/to/file · save · split-right · terminal";onAccepted:palette.accept()}
            Label {Layout.fillWidth:true;wrapMode:Text.Wrap;text:"find TEXT · replace TEXT => VALUE · replace-all TEXT => VALUE · goto LINE · indent · outdent · set OPTION VALUE · settings-reload · open PATH · save · save-as PATH · new · close · undo · redo · split-right · split-down · terminal · terminate-terminal · files · git · editor · close-pane · move-pane ID · preset development|minimal|bottom_terminal · layout-save NAME · layout-load NAME · refresh · stage · unstage · diff · commit MESSAGE · quit · discard-quit · discard-document"}
        }
    }
    Dialog {
        id:quitDialog
        title:"Unsaved documents"
        anchors.centerIn:parent
        modal:true
        standardButtons:Dialog.Discard|Dialog.Cancel
        Label {text:"Discard all unsaved changes and quit?"}
        onDiscarded:root.send({action:"quit",force:true})
    }
}
