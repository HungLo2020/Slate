// Built only with the gui-smoke Cargo feature. No test machinery in normal builds.
#include "bridge.h"
#include <QTest>
#include <QQuickWindow>
#include <QTimer>
#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QJsonObject>
#include <QGuiApplication>
#include <functional>

static QQuickItem *findItem(QQuickItem *item,const QString &name){
    if(item->objectName()==name)return item;
    for(auto child:item->childItems())if(auto found=findItem(child,name))return found;
    return nullptr;
}
static QByteArray read(const QString &path){QFile file(path);if(!file.open(QIODevice::ReadOnly))return {};return file.readAll();}
void startSmoke(Bridge *state,QQuickWindow *window){
    const QString dir=qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer=new QTimer(state);auto step=new int(0);auto ticks=new int(0);
    auto finish=[=](bool pass,const QString &detail){
        QFile report(dir+"/report.json");report.open(QIODevice::WriteOnly);report.write(QJsonDocument(QJsonObject{{"pass",pass},{"detail",detail},{"steps",*step}}).toJson());
        window->grabWindow().save(dir+"/gui.png");timer->stop();QGuiApplication::exit(pass?0:2);
    };
    auto key=[=](Qt::Key key,Qt::KeyboardModifiers modifiers=Qt::NoModifier){QTest::keyClick(window,key,modifiers);};
    auto type=[=](const QString &text){for(auto c:text)QTest::keyClick(window,c.toLatin1());};
    auto command=[=](const QString &text){key(Qt::Key_F1);type(text);key(Qt::Key_Return);};
    auto click=[=](QQuickItem *item,const QPointF &position,bool twice){auto p=item->mapToScene(position).toPoint();if(twice)QTest::mouseDClick(window,Qt::LeftButton,Qt::NoModifier,p);else QTest::mouseClick(window,Qt::LeftButton,Qt::NoModifier,p);};
    QObject::connect(timer,&QTimer::timeout,state,[=](){
        state->refresh();if(++*ticks>150){finish(false,QString("Timed out at step %1: %2").arg(*step).arg(state->frame().value("status").toString()));return;}
        auto frame=state->frame();
        switch(*step){
        case 0:{
            if(state->files().size()<3)return;
            if(state->paneIds().size()!=3){finish(false,"Default workspace did not contain three panes");return;}
            auto browser=findItem(window->contentItem(),"browser_1");if(!browser){finish(false,"Missing file browser");return;}
            click(browser,QPointF(60,28+14),true);break;
        }
        case 1:{
            auto grid=findItem(window->contentItem(),"cells_2");if(!grid){finish(false,"Missing editor");return;}
            grid->forceActiveFocus();key(Qt::Key_A,Qt::ControlModifier);type("GUI edited");key(Qt::Key_Return);type("second line");key(Qt::Key_S,Qt::ControlModifier);break;
        }
        case 2:
            if(read(dir+"/workspace/edit.txt")!="GUI edited\nsecond line"){finish(false,"GUI edit/save did not write expected bytes");return;}
            key(Qt::Key_Z,Qt::ControlModifier);key(Qt::Key_Y,Qt::ControlModifier);key(Qt::Key_S,Qt::ControlModifier);
            command("split-down");break;
        case 3:
            if(state->paneIds().size()!=4){finish(false,"Split command did not create a pane");return;}
            command("terminal");break;
        case 4:{
            const auto panes=frame.value("panes").toList();bool terminal=false;for(const auto &p:panes)if(p.toMap().value("id")==frame.value("focus"))terminal=p.toMap().value("kind")=="terminal";
            if(!terminal){finish(false,"New terminal did not become active");return;}
            auto grid=findItem(window->contentItem(),"cells_"+frame.value("focus").toString());if(!grid){finish(false,"Missing terminal presentation");return;}grid->forceActiveFocus();
            type("printf 'GUI_PTY_OK' > terminal.txt");key(Qt::Key_Return);break;
        }
        case 5:
            if(read(dir+"/workspace/terminal.txt")!="GUI_PTY_OK")return;
            command("layout-save smoke");command("preset minimal");break;
        case 6:
            if(state->paneIds().size()!=1){finish(false,"Minimal preset did not collapse the layout");return;}
            command("layout-load smoke");break;
        case 7:
            if(state->paneIds().size()!=4){finish(false,"Saved layout did not restore split panes");return;}
            if(!QFile::exists(dir+"/config/slate/layouts.toml")){finish(false,"Layout configuration was not persisted");return;}
            command("preset development");command("refresh");break;
        case 8:{
            auto browser=findItem(window->contentItem(),"browser_1");if(!browser){finish(false,"Missing restored browser");return;}
            int index=-1;auto files=state->files();for(int i=0;i<files.size();++i)if(files[i].toMap().value("name")=="second.txt")index=i;
            if(index<0)return;click(browser,QPointF(60,index*28+14),true);break;
        }
        case 9:{
            auto grid=findItem(window->contentItem(),"cells_2");grid->forceActiveFocus();key(Qt::Key_A,Qt::ControlModifier);type("SECOND_OK");key(Qt::Key_S,Qt::ControlModifier);break;
        }
        case 10:
            if(read(dir+"/workspace/second.txt")!="SECOND_OK"){finish(false,"Double-click file browser did not open/edit the second document");return;}
            finish(true,"Three panes, real GUI file browsing, editing/save, undo/redo, shared split, PTY command, layout persistence");return;
        }
        ++*step;
    });timer->start(100);
}
