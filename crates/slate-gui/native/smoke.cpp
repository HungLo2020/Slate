// Built only with the gui-smoke Cargo feature. No test machinery in normal builds.
#include "bridge.h"
#include <QClipboard>
#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QJsonDocument>
#include <QJsonObject>
#include <QQuickWindow>
#include <QTest>
#include <QTimer>
#include <functional>

static QQuickItem *findItem(QQuickItem *item, const QString &name) {
    if (item->objectName() == name)
        return item;
    for (auto child : item->childItems())
        if (auto found = findItem(child, name))
            return found;
    return nullptr;
}
static QByteArray read(const QString &path) {
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly))
        return {};
    return file.readAll();
}
void startSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto step = new int(0);
    auto ticks = new int(0);
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json");
        report.open(QIODevice::WriteOnly);
        report.write(
            QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}, {"steps", *step}})
                .toJson());
        window->grabWindow().save(dir + "/gui.png");
        timer->stop();
        QGuiApplication::exit(pass ? 0 : 2);
    };
    auto key = [=](Qt::Key key, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
        QTest::keyClick(window, key, modifiers);
    };
    auto type = [=](const QString &text) {
        for (auto c : text)
            QTest::keyClick(window, c.toLatin1());
    };
    auto command = [=](const QString &text) {
        key(Qt::Key_F1);
        type(text);
        key(Qt::Key_Return);
    };
    auto click = [=](QQuickItem *item, const QPointF &position, bool twice) {
        auto p = item->mapToScene(position).toPoint();
        if (twice)
            QTest::mouseDClick(window, Qt::LeftButton, Qt::NoModifier, p);
        else
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier, p);
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        state->refresh();
        if (++*ticks > 150) {
            finish(false, QString("Timed out at step %1: %2")
                              .arg(*step)
                              .arg(state->frame().value("status").toString()));
            return;
        }
        auto frame = state->frame();
        switch (*step) {
        case 0: {
            if (state->files().size() < 3)
                return;
            if (state->paneIds().size() != 3) {
                finish(false, "Default workspace did not contain three panes");
                return;
            }
            auto browser = findItem(window->contentItem(), "browser_1");
            if (!browser) {
                finish(false, "Missing file browser");
                return;
            }
            click(browser, QPointF(60, 28 + 14), true);
            break;
        }
        case 1: {
            auto grid = findItem(window->contentItem(), "cells_2");
            if (!grid) {
                finish(false, "Missing editor");
                return;
            }
            grid->forceActiveFocus();
            key(Qt::Key_A, Qt::ControlModifier);
            type("GUI edited");
            key(Qt::Key_Return);
            type("second line");
            key(Qt::Key_S, Qt::ControlModifier);
            break;
        }
        case 2:
            if (read(dir + "/workspace/edit.txt") != "GUI edited\nsecond line") {
                finish(false, "GUI edit/save did not write expected bytes");
                return;
            }
            key(Qt::Key_Z, Qt::ControlModifier);
            key(Qt::Key_Y, Qt::ControlModifier);
            key(Qt::Key_S, Qt::ControlModifier);
            command("split-down");
            break;
        case 3:
            if (state->paneIds().size() != 4) {
                finish(false, "Split command did not create a pane");
                return;
            }
            command("terminal");
            break;
        case 4: {
            const auto panes = frame.value("panes").toList();
            bool terminal = false;
            for (const auto &p : panes)
                if (p.toMap().value("id") == frame.value("focus"))
                    terminal = p.toMap().value("kind") == "terminal";
            if (!terminal) {
                finish(false, "New terminal did not become active");
                return;
            }
            auto grid = findItem(window->contentItem(), "cells_" + frame.value("focus").toString());
            if (!grid) {
                finish(false, "Missing terminal presentation");
                return;
            }
            grid->forceActiveFocus();
            type("printf 'GUI_PTY_OK' > terminal.txt");
            key(Qt::Key_Return);
            break;
        }
        case 5:
            if (read(dir + "/workspace/terminal.txt") != "GUI_PTY_OK")
                return;
            command("layout-save smoke");
            command("preset minimal");
            break;
        case 6:
            if (state->paneIds().size() != 1) {
                finish(false, "Minimal preset did not collapse the layout");
                return;
            }
            command("layout-load smoke");
            break;
        case 7:
            if (state->paneIds().size() != 4) {
                finish(false, "Saved layout did not restore split panes");
                return;
            }
            if (!QFile::exists(dir + "/config/slate/layouts.toml")) {
                finish(false, "Layout configuration was not persisted");
                return;
            }
            command("preset development");
            command("refresh");
            break;
        case 8: {
            auto browser = findItem(window->contentItem(), "browser_1");
            if (!browser) {
                finish(false, "Missing restored browser");
                return;
            }
            int index = -1;
            auto files = state->files();
            for (int i = 0; i < files.size(); ++i)
                if (files[i].toMap().value("name") == "second.txt")
                    index = i;
            if (index < 0)
                return;
            click(browser, QPointF(60, index * 28 + 14), true);
            break;
        }
        case 9: {
            auto grid = findItem(window->contentItem(), "cells_2");
            grid->forceActiveFocus();
            key(Qt::Key_A, Qt::ControlModifier);
            type("SECOND_OK");
            key(Qt::Key_S, Qt::ControlModifier);
            break;
        }
        case 10:
            if (read(dir + "/workspace/second.txt") != "SECOND_OK") {
                finish(false, "Double-click file browser did not open/edit the second document");
                return;
            }
            key(Qt::Key_F, Qt::ControlModifier);
            break;
        case 11: {
            auto input = findItem(window->contentItem(), "searchInput");
            if (!input)
                return;
            input->forceActiveFocus();
            type("SECOND");
            key(Qt::Key_Return);
            break;
        }
        case 12: {
            if (!frame.value("status").toString().startsWith("Match 1 of 1")) {
                finish(false, "Find dialog did not select expected text");
                return;
            }
            key(Qt::Key_Escape);
            key(Qt::Key_H, Qt::ControlModifier);
            break;
        }
        case 13: {
            auto input = findItem(window->contentItem(), "replacementInput");
            if (!input)
                return;
            input->forceActiveFocus();
            type("NEW");
            key(Qt::Key_Return);
            key(Qt::Key_Escape);
            key(Qt::Key_S, Qt::ControlModifier);
            break;
        }
        case 14:
            if (read(dir + "/workspace/second.txt") != "NEW_OK") {
                finish(false, "Replace dialog did not edit the selected match");
                return;
            }
            command("set indent-width 2");
            key(Qt::Key_G, Qt::ControlModifier);
            break;
        case 15: {
            auto input = findItem(window->contentItem(), "searchInput");
            if (!input)
                return;
            input->forceActiveFocus();
            type("1");
            key(Qt::Key_Return);
            key(Qt::Key_Tab);
            key(Qt::Key_S, Qt::ControlModifier);
            break;
        }
        case 16:
            if (read(dir + "/workspace/second.txt") != "  NEW_OK") {
                finish(false, "Go-to-line / indentation settings did not reach editor input");
                return;
            }
            command("terminal");
            break;
        case 17: {
            auto grid = findItem(window->contentItem(), "cells_" + frame.value("focus").toString());
            if (!grid)
                return;
            grid->forceActiveFocus();
            type("printf '\\033[2J\\033[HGUI_COPY_TEXT\\n'");
            key(Qt::Key_Return);
            break;
        }
        case 18: {
            auto grid = findItem(window->contentItem(), "cells_" + frame.value("focus").toString());
            if (!grid)
                return;
            auto start = grid->mapToScene(QPointF(2, 2)).toPoint();
            auto end = grid->mapToScene(QPointF(2 + 13 * state->cellWidth(), 2)).toPoint();
            QTest::mousePress(window, Qt::LeftButton, Qt::ShiftModifier, start);
            QTest::mouseMove(window, end);
            QTest::mouseRelease(window, Qt::LeftButton, Qt::ShiftModifier, end);
            key(Qt::Key_C, Qt::ControlModifier | Qt::ShiftModifier);
            break;
        }
        case 19:
            if (QGuiApplication::clipboard()->text() != "GUI_COPY_TEXT") {
                finish(false, "Terminal selection did not copy to system clipboard");
                return;
            }
            QGuiApplication::clipboard()->setText("printf 'PASTE_OK' > paste.txt");
            key(Qt::Key_V, Qt::ControlModifier | Qt::ShiftModifier);
            key(Qt::Key_Return);
            break;
        case 20:
            if (read(dir + "/workspace/paste.txt") != "PASTE_OK")
                return;
            finish(true,
                   "Three panes, file browsing, edit/save, undo/redo, splits/layouts, find/replace "
                   "dialogs, go-to-line, settings, PTY selection/system clipboard/paste");
            return;
        }
        ++*step;
    });
    timer->start(100);
}
