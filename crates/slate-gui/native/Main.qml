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
    function send(action) { slate.send(action); slate.refresh() }
    function pane(id) { var items = frame.panes || []; for (var i=0;i<items.length;i++) if(items[i].id===id) return items[i]; return ({id:id,kind:"files",rect:{x:0,y:0,width:0,height:0},tabs:[]}) }
    function handle(id) { var items=frame.handles||[];for(var i=0;i<items.length;i++)if(items[i].id===id)return items[i];return ({id:id,axis:"horizontal",rect:{x:0,y:0,width:0,height:0},parent:{x:0,y:0,width:1,height:1}}) }
    function focusPane() { for(var i=0;i<panes.count;i++){var item=panes.itemAt(i);if(item&&item.paneId===frame.focus)item.focusContent()} }
    function paletteWith(text) { commandText.text=text || ""; palette.open(); commandText.forceActiveFocus(); commandText.cursorPosition=commandText.text.length }
    onClosing: function(close) {close.accepted=false;slate.exit();if(slate.frame.dirty)quitDialog.open()}
    Connections {
        target: slate
        function onFrameChanged() { if(root.priorFocus!==root.frame.focus){root.priorFocus=root.frame.focus;Qt.callLater(root.focusPane)} }
    }
    Shortcut { sequence: "F1"; onActivated: root.paletteWith("") }
    Shortcut { sequence: "Ctrl+Shift+P"; onActivated: root.paletteWith("") }
    Shortcut { sequence: "F6"; onActivated: root.send({action:"focus_next"}) }
    Shortcut { sequence: "F7"; onActivated: root.send({action:"next_tab"}) }
    Shortcut { sequence: "F8"; onActivated: root.send({action:"new_terminal"}) }
    Shortcut { sequence: "F9"; onActivated: root.send({action:"split",axis:"horizontal"}) }
    Shortcut { sequence: "Shift+F9"; onActivated: root.send({action:"split",axis:"vertical"}) }
    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            ToolButton {text:"Open…";onClicked:root.paletteWith("open ")}
            ToolButton {text:"New";onClicked:root.send({action:"new"})}
            ToolButton {text:"Save";onClicked:root.send({action:"save"})}
            ToolButton {text:"Save as…";onClicked:root.paletteWith("save-as ")}
            ToolButton {text:"Split ▸";onClicked:root.send({action:"split",axis:"horizontal"})}
            ToolButton {text:"Split ▾";onClicked:root.send({action:"split",axis:"vertical"})}
            ToolButton {text:"Terminal +";onClicked:root.send({action:"new_terminal"})}
            ToolButton {text:"Layout";onClicked:layoutMenu.open()}
            Item {Layout.fillWidth:true}
            ToolButton {text:"Commands";onClicked:root.paletteWith("")}
            ToolButton {text:"Quit";onClicked:root.close()}
        }
        Menu {
            id:layoutMenu
            MenuItem {text:"Three panes";onTriggered:root.send({action:"preset",name:"development"})}
            MenuItem {text:"Terminal below";onTriggered:root.send({action:"preset",name:"bottom_terminal"})}
            MenuItem {text:"Editor only";onTriggered:root.send({action:"preset",name:"minimal"})}
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
                color:"#20242c"
                border.color:root.frame.focus===paneId?"#88c0d0":"#434c5e"
                border.width:1
                clip:true
                function focusContent(){if(paneData.kind==="editor"||paneData.kind==="terminal")grid.forceActiveFocus();else browser.forceActiveFocus()}
                Row {
                    id:tabs
                    height:30;width:parent.width
                    Repeater {
                        model:panel.paneData.tabs
                        delegate:Button {
                            required property var modelData
                            required property int index
                            text:modelData.title
                            height:30;implicitWidth:Math.min(180,Math.max(65,implicitContentWidth+24))
                            highlighted:modelData.active
                            onClicked:{root.send({action:"switch_tab",pane:panel.paneId,index:index});panel.focusContent()}
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
                    Keys.onPressed:function(event){
                        var key=event.key===Qt.Key_Up?"Up":event.key===Qt.Key_Down?"Down":event.key===Qt.Key_Return?"Enter":"";
                        if(key){root.send({action:"focus",pane:panel.paneId});root.send({action:"key",key:key});event.accepted=true}
                    }
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
                MouseArea {anchors.fill:parent;acceptedButtons:Qt.RightButton;onClicked:{root.send({action:"focus",pane:panel.paneId});paneMenu.open()}}
                Menu {
                    id:paneMenu
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
                Label {anchors.right:parent.right;anchors.bottom:parent.bottom;text:"#"+panel.paneId;font.pixelSize:10;color:"#68778d"}
            }
        }
        Repeater {
            model:slate.handleIds
            delegate:Rectangle {
                required property var modelData
                property var handleData:root.handle(modelData)
                x:handleData.rect.x;y:handleData.rect.y;width:handleData.rect.width;height:handleData.rect.height
                color:drag.containsMouse?"#88c0d0":"#434c5e"
                MouseArea {
                    id:drag;anchors.fill:parent;hoverEnabled:true
                    cursorShape:parent.handleData.axis==="horizontal"?Qt.SplitHCursor:Qt.SplitVCursor
                    onPositionChanged:function(mouse){if(pressed){var p=mapToItem(workspace,mouse.x,mouse.y);var h=parent.handleData;var ratio=h.axis==="horizontal"?(p.x-h.parent.x)/h.parent.width:(p.y-h.parent.y)/h.parent.height;root.send({action:"resize_split",id:h.id,ratio:ratio})}}
                }
            }
        }
    }
    footer:ToolBar {Label {anchors.fill:parent;anchors.margins:6;text:(root.frame.status||"")+"    · F1 commands · F6 pane · F7 tab";elide:Text.ElideRight}}
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
            Label {Layout.fillWidth:true;wrapMode:Text.Wrap;text:"open PATH · save · save-as PATH · new · close · undo · redo · split-right · split-down · terminal · terminate-terminal · files · git · editor · close-pane · move-pane ID · preset development|minimal|bottom_terminal · layout-save NAME · layout-load NAME · refresh · stage · unstage · diff · commit MESSAGE · quit · discard-quit · discard-document"}
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
