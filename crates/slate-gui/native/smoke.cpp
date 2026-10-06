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
static QString checkCaptions(Bridge *state, QQuickWindow *window) {
    for (const auto &entry : state->frame().value("panes").toList()) {
        const auto pane = entry.toMap();
        const auto id = pane.value("id").toString();
        auto tabs = findItem(window->contentItem(), "tabs_" + id);
        if (!tabs)
            return "Missing pane tabs";
        const auto entries = pane.value("tabs").toList();
        for (int index = 0; index < entries.size(); ++index) {
            auto tab = findItem(tabs, "tab_" + id + "_" + QString::number(index));
            // ListView may not instantiate tabs outside the visible viewport.
            if (!tab)
                continue;
            auto background = tab->property("background").value<QQuickItem *>();
            auto content = tab->property("contentItem").value<QQuickItem *>();
            if (background && content && !background->property("text").toString().isEmpty() &&
                !content->property("text").toString().isEmpty())
                return "Pane tab paints captions in both its background and content";
        }
    }
    return {};
}
static QString checkLayout(Bridge *state, QQuickWindow *window) {
    auto actions = findItem(window->contentItem(), "mainActions");
    if (!actions)
        return "Missing toolbar";
    QList<QRectF> controls;
    for (auto item : actions->childItems()) {
        if (!item->isVisible() || item->objectName().isEmpty())
            continue;
        const QRectF rect(item->x(), item->y(), item->width(), item->height());
        if (rect.left() < -1 || rect.top() < -1 || rect.right() > actions->width() + 1 ||
            rect.bottom() > actions->height() + 1)
            return "Toolbar control outside its layout: " + item->objectName();
        if (item->height() + 1 < item->implicitHeight() ||
            item->width() + 1 < item->implicitWidth())
            return QString("Toolbar label clipped: %1 (%2x%3, implicit %4x%5)")
                .arg(item->objectName())
                .arg(item->width())
                .arg(item->height())
                .arg(item->implicitWidth())
                .arg(item->implicitHeight());
        for (const auto &other : controls)
            if (rect.intersects(other))
                return "Toolbar controls overlap";
        controls.append(rect);
    }
    const auto panes = state->frame().value("panes").toList();
    const int header = window->property("paneHeaderHeight").toInt();
    for (const auto &entry : panes) {
        const auto pane = entry.toMap();
        const auto id = pane.value("id").toString();
        auto tabs = findItem(window->contentItem(), "tabs_" + id);
        auto button = findItem(window->contentItem(), "paneActions_" + id);
        auto chrome = findItem(window->contentItem(), "paneHeader_" + id);
        if (!tabs || !button || !chrome || tabs->width() < 0 ||
            tabs->x() + tabs->width() > button->x() + 1 ||
            button->x() + button->width() > chrome->width() + 1)
            return "Pane tabs/actions overlap or escape their header";
        if (chrome->height() != header || button->height() < button->implicitHeight() - 1)
            return "Pane header clips its controls";
        auto grid = findItem(window->contentItem(), "cells_" + id);
        if (grid && grid->isVisible()) {
            if (grid->y() < chrome->y() + chrome->height() || grid->height() < 0)
                return "Text grid overlaps pane header";
            const auto screen = pane.value("screen").toMap();
            if (screen.value("rows").toInt() * state->cellHeight() > grid->height())
                return "Editor/PTY rows extend behind the pane boundary";
        }
    }
    return checkCaptions(state, window);
}
static void startLayoutSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    const auto longPath =
        dir + "/workspace/a_very_long_document_name_that_requires_tab_elision_and_a_tooltip.rs";
    QFile longFile(longPath);
    longFile.open(QIODevice::WriteOnly);
    longFile.write("// layout test\n");
    longFile.close();
    state->command("open " + longPath);
    const QList<QSize> sizes{{480, 320}, {800, 600}, {1360, 820}, {1920, 1080}};
    const QList<int> fonts{10, 16, 24};
    auto timer = new QTimer(state);
    auto phase = new int(0);
    auto finish = [=](const QString &error) {
        QFile report(dir + "/report.json");
        report.open(QIODevice::WriteOnly);
        report.write(
            QJsonDocument(
                QJsonObject{{"pass", error.isEmpty()},
                            {"detail", error.isEmpty()
                                           ? "12 window/font combinations: toolbar bounds, pane "
                                             "actions, measured headers and PTY geometry"
                                           : error}})
                .toJson());
        timer->stop();
        QGuiApplication::exit(error.isEmpty() ? 0 : 2);
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        const int scenario = *phase / 4;
        if (scenario >= sizes.size() * fonts.size()) {
            finish({});
            return;
        }
        if (*phase % 4 == 0) {
            QFont font = QGuiApplication::font();
            font.setPointSize(fonts[scenario / sizes.size()]);
            window->setProperty("font", font);
            window->resize(sizes[scenario % sizes.size()]);
        } else if (*phase % 4 == 1) {
            state->refresh();
            // Rendering polishes layouts after font/size changes before we inspect geometry.
            window->grabWindow().save(dir + QString("/layout-%1.png").arg(scenario));
            const auto error = checkLayout(state, window);
            if (!error.isEmpty()) {
                finish(QString("%1 at %2x%3, font %4")
                           .arg(error)
                           .arg(window->width())
                           .arg(window->height())
                           .arg(fonts[scenario / sizes.size()]));
                return;
            }
            if (scenario == 2)
                window->grabWindow().save(dir + "/gui.png");
            QTest::keyClick(window, Qt::Key_F1);
        } else {
            const auto name = *phase % 4 == 2 ? "commandPalette" : "editPrompt";
            auto popup = window->findChild<QObject *>(name);
            if (!popup || !popup->property("visible").toBool() ||
                popup->property("height").toReal() > window->height() - 39 ||
                popup->property("width").toReal() > window->width() - 39 ||
                popup->property("availableHeight").toReal() <= 0) {
                finish(QString("Dialog bounds/visibility failed for %1 at scenario %2")
                           .arg(name)
                           .arg(scenario));
                return;
            }
            window->grabWindow().save(dir + QString("/dialog-%1-%2.png").arg(scenario).arg(name));
            QTest::keyClick(window, Qt::Key_Escape);
            if (*phase % 4 == 2) {
                state->send({{"action", "prompt"}, {"kind", "replace"}});
                state->refresh();
            }
        }
        ++*phase;
    });
    timer->start(150);
}

void startSmoke(Bridge *state, QQuickWindow *window) {
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_LAYOUT_SMOKE")) {
        startLayoutSmoke(state, window);
        return;
    }
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
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_CAPTION_SMOKE")) {
        QTimer::singleShot(500, state, [=]() {
            state->refresh();
            const auto error = checkCaptions(state, window);
            finish(error.isEmpty(),
                   error.isEmpty() ? "Pane tabs have a single caption renderer" : error);
        });
        return;
    }
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
            click(browser, QPointF(60, window->property("fileRowHeight").toInt() * 1.5), true);
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
            click(browser, QPointF(60, (index + 0.5) * window->property("fileRowHeight").toInt()),
                  true);
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
