#include "bridge.h"
#include <QGuiApplication>
#include <QApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickWindow>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTimer>
#include <QFontDatabase>
#include <QFontMetrics>
#include <QClipboard>
#include <QPainter>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QWheelEvent>
#include <QInputMethodEvent>
#include <cmath>
extern "C" char *slate_request(void *,const char *);
extern "C" void slate_response_free(char *);
static Bridge *bridge=nullptr;
#ifdef SLATE_SMOKE_TEST
void startSmoke(Bridge *, QQuickWindow *);
#endif
Bridge::Bridge(void *context,QObject *parent):QObject(parent),m_context(context){}
QFont Bridge::font() const {auto f=QFontDatabase::systemFont(QFontDatabase::FixedFont);f.setPointSize(11);f.setStyleHint(QFont::Monospace);return f;}
int Bridge::cellWidth() const {return int(std::ceil(QFontMetricsF(font()).horizontalAdvance("M")));}
int Bridge::cellHeight() const {return int(std::ceil(QFontMetricsF(font()).height()));}
QVariantMap Bridge::send(const QVariantMap &command) {
    const auto bytes=QJsonDocument(QJsonObject::fromVariantMap(command)).toJson(QJsonDocument::Compact);
    char *raw=slate_request(m_context,bytes.constData());
    const auto result=QJsonDocument::fromJson(QByteArray(raw)).object().toVariantMap();slate_response_free(raw);
    return result;
}
void Bridge::command(const QString &text){send({{"action","command"},{"text",text}});refresh();}
void Bridge::viewport(int width,int height){m_width=qBound(1,width,65535);m_height=qBound(1,height,65535);refresh();}
void Bridge::refresh(){
    const auto frame=send({{"action","snapshot"},{"width",m_width},{"height",m_height},{"cell_width",cellWidth()},{"cell_height",cellHeight()}});
    if(frame!=m_frame){
        QVariantList panes,handles;
        for(const auto &p:frame.value("panes").toList())panes.append(p.toMap().value("id"));
        for(const auto &h:frame.value("handles").toList())handles.append(h.toMap().value("id"));
        const bool structure=panes!=m_paneIds||handles!=m_handleIds;
        const bool fileChange=frame.value("files")!=m_frame.value("files"),gitChange=frame.value("git")!=m_frame.value("git");
        m_frame=frame;m_paneIds=panes;m_handleIds=handles;
        if(structure)emit structureChanged();if(fileChange)emit filesChanged();if(gitChange)emit gitChanged();emit frameChanged();
    }
    if(frame.value("quit").toBool())QCoreApplication::quit();
}
void Bridge::copyClipboard(){send({{"action","copy"}});const auto response=send({{"action","snapshot"},{"width",m_width},{"height",m_height},{"cell_width",cellWidth()},{"cell_height",cellHeight()}});QGuiApplication::clipboard()->setText(response.value("clipboard").toString());refresh();}
void Bridge::pasteClipboard(){send({{"action","paste"},{"text",QGuiApplication::clipboard()->text()}});refresh();}
void Bridge::exit(){send({{"action","quit"},{"force",false}});refresh();}
CellView::CellView(QQuickItem *parent):QQuickPaintedItem(parent){setAcceptedMouseButtons(Qt::LeftButton);setFlag(ItemAcceptsInputMethod,true);setActiveFocusOnTab(true);setClip(true);}
void CellView::setPane(const QVariantMap &pane){if(m_pane==pane)return;m_pane=pane;emit paneChanged();update();}
void CellView::paint(QPainter *p){
    if(!bridge)return;
    p->fillRect(boundingRect(),QColor("#20242c"));
    const auto screen=m_pane.value("screen").toMap();const auto rows=screen.value("cells").toList();
    const int cw=bridge->cellWidth(),ch=bridge->cellHeight();
    const auto base=bridge->font();const auto ascent=QFontMetrics(base).ascent();
    for(int row=0;row<rows.size();++row){const auto line=rows[row].toList();for(int col=0;col<line.size();++col){
        const auto cell=line[col].toMap();QRect rect(1+col*cw,1+row*ch,cw,ch);
        p->fillRect(rect,QColor(cell.value("bg").toString()));
        if(cell.value("continuation").toBool())continue;
        auto f=base;f.setBold(cell.value("bold").toBool());f.setItalic(cell.value("italic").toBool());f.setUnderline(cell.value("underline").toBool());p->setFont(f);p->setPen(QColor(cell.value("fg").toString()));
        p->drawText(rect.x(),rect.y()+ascent,cell.value("text").toString());
    }}
    const auto cursor=screen.value("cursor").toList();
    if(cursor.size()==2 && bridge->frame().value("focus")==m_pane.value("id")){
        p->setPen(QColor("#88c0d0"));p->drawRect(1+cursor[1].toInt()*cw,1+cursor[0].toInt()*ch,cw-1,ch-1);
    }
}
static QString keyName(QKeyEvent *e){
    switch(e->key()){
    case Qt::Key_Return:case Qt::Key_Enter:return "Enter";case Qt::Key_Backspace:return "Backspace";case Qt::Key_Delete:return "Delete";case Qt::Key_Insert:return "Insert";
    case Qt::Key_Up:return "Up";case Qt::Key_Down:return "Down";case Qt::Key_Left:return "Left";case Qt::Key_Right:return "Right";
    case Qt::Key_Home:return "Home";case Qt::Key_End:return "End";case Qt::Key_PageUp:return "PageUp";case Qt::Key_PageDown:return "PageDown";
    case Qt::Key_Tab:case Qt::Key_Backtab:return "Tab";case Qt::Key_Escape:return "Escape";
    default:if(e->key()>=Qt::Key_F1&&e->key()<=Qt::Key_F12)return QString("F%1").arg(e->key()-Qt::Key_F1+1);
    if(e->key()>=Qt::Key_A&&e->key()<=Qt::Key_Z)return QChar(e->key()).toLower();return e->text();
    }
}
void CellView::keyPressEvent(QKeyEvent *e){
    if(!bridge)return;
    const bool ctrl=e->modifiers().testFlag(Qt::ControlModifier),shift=e->modifiers().testFlag(Qt::ShiftModifier);
    if(ctrl && e->key()==Qt::Key_V){bridge->pasteClipboard();e->accept();return;}
    if(ctrl && e->key()==Qt::Key_C && m_pane.value("kind")=="editor"){bridge->copyClipboard();e->accept();return;}
    bridge->send({{"action","key"},{"key",keyName(e)},{"text",e->text()},{"ctrl",ctrl},{"alt",e->modifiers().testFlag(Qt::AltModifier)},{"shift",shift||e->key()==Qt::Key_Backtab}});bridge->refresh();e->accept();
}
void CellView::click(QMouseEvent *e,bool extend){
    forceActiveFocus();bridge->send({{"action","click"},{"pane",m_pane.value("id")},{"row",qMax(0,int(e->position().y()-1)/bridge->cellHeight())},{"col",qMax(0,int(e->position().x()-1)/bridge->cellWidth())},{"shift",extend||e->modifiers().testFlag(Qt::ShiftModifier)}});bridge->refresh();
}
void CellView::mousePressEvent(QMouseEvent *e){click(e,false);e->accept();}
void CellView::mouseMoveEvent(QMouseEvent *e){if(e->buttons().testFlag(Qt::LeftButton))click(e,true);e->accept();}
void CellView::wheelEvent(QWheelEvent *e){bridge->send({{"action","scroll"},{"pane",m_pane.value("id")},{"delta",e->angleDelta().y()/40}});bridge->refresh();e->accept();}
void CellView::inputMethodEvent(QInputMethodEvent *e){if(!e->commitString().isEmpty())bridge->send({{"action","paste"},{"text",e->commitString()}});bridge->refresh();e->accept();}
QVariant CellView::inputMethodQuery(Qt::InputMethodQuery query)const{
    if(query==Qt::ImEnabled)return true;
    if(query==Qt::ImCursorRectangle){const auto cursor=m_pane.value("screen").toMap().value("cursor").toList();if(cursor.size()==2)return QRectF(1+cursor[1].toInt()*bridge->cellWidth(),1+cursor[0].toInt()*bridge->cellHeight(),bridge->cellWidth(),bridge->cellHeight());}
    return QQuickPaintedItem::inputMethodQuery(query);
}
static void initializeResources(){Q_INIT_RESOURCE(resources);}
extern "C" int slate_qt_run(void *context){
    int argc=1;char name[]="slate-gui";char *argv[]={name,nullptr};
    QApplication app(argc,argv);app.setApplicationName("Slate");app.setOrganizationName("Slate");
    initializeResources();
    Bridge state(context);bridge=&state;
    qmlRegisterType<CellView>("Slate.Native",1,0,"CellView");
    QQmlApplicationEngine engine;engine.rootContext()->setContextProperty("slate",&state);
    engine.load(QUrl("qrc:/slate/Main.qml"));
    if(engine.rootObjects().isEmpty()){bridge=nullptr;return 1;}
    QTimer timer;QObject::connect(&timer,&QTimer::timeout,&state,&Bridge::refresh);timer.start(33);state.refresh();
    #ifdef SLATE_SMOKE_TEST
    if(!qEnvironmentVariableIsEmpty("SLATE_GUI_SMOKE_DIR"))startSmoke(&state,qobject_cast<QQuickWindow *>(engine.rootObjects().first()));
#endif
    const int result=app.exec();bridge=nullptr;return result;
}
