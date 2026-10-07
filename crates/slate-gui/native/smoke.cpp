// Built only with the gui-smoke Cargo feature. No test machinery in normal builds.
#include "bridge.h"
#include <QApplication>
#include <QAbstractButton>
#include <QComboBox>
#include <QFileDialog>
#include <QFileInfo>
#include <QIcon>
#include <QClipboard>
#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QInputMethodEvent>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLineEdit>
#include <QMessageBox>
#include <QPointer>
#include <QPushButton>
#include <QQuickWindow>
#include <QSignalSpy>
#include <QSignalBlocker>
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
static QQuickItem *findMenuItem(QQuickWindow *window, const QString &name) {
    auto item = findItem(window->contentItem(), name);
    if (item && item->isVisible())
        return item;
    // Desktop styles can place menus in separate popup windows.
    for (auto candidate : QGuiApplication::allWindows())
        if (auto popup = qobject_cast<QQuickWindow *>(candidate))
            if (popup != window && popup->isVisible())
                if (auto found = findItem(popup->contentItem(), name))
                    return found;
    return item;
}
static void dismissMenu(QQuickWindow *window) {
    auto popup = QGuiApplication::focusWindow();
    QTest::keyClick(popup ? popup : window, Qt::Key_Escape);
}
static bool openMenu(QQuickWindow *window, QTimer *driver, const QString &name) {
    // QtTest injects input faster than desktop menu animations. Block the
    // workflow timer while waiting so its next step cannot run recursively.
    QSignalBlocker block(driver);
    const QStringList names{"fileMenu", "editMenu", "viewMenu", "goMenu", "terminalMenu"};
    for (const auto &name : names)
        if (auto menu = window->findChild<QObject *>(name))
            if (menu->property("visible").toBool()) QMetaObject::invokeMethod(menu, "close");
    if (!QTest::qWaitFor([=]() {
        for (const auto &name : names)
            if (auto menu = window->findChild<QObject *>(name))
                if (menu->property("visible").toBool()) return false;
        return true;
    }, 2000)) return false;
    auto heading = findItem(window->contentItem(), "menu_" + name);
    auto menu = window->findChild<QObject *>(name.toLower() + "Menu");
    if (!heading || !heading->isVisible() || !menu) return false;
    QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier,
        heading->mapToScene(QPointF(heading->width()/2, heading->height()/2)).toPoint());
    return QTest::qWaitFor([=]() { return menu->property("opened").toBool(); }, 2000);
}
static bool clickMenuAction(QQuickWindow *window, QTimer *driver, const QString &menu, const QString &action) {
    QSignalBlocker block(driver);
    if (!openMenu(window, driver, menu)) return false;
    auto item = findMenuItem(window, "menuAction_" + action);
    if (!item || !item->isVisible() || !item->isEnabled()) return false;
    QSignalSpy triggered(item, SIGNAL(triggered()));
    QTest::mouseClick(item->window(), Qt::LeftButton, Qt::NoModifier,
        item->mapToScene(QPointF(item->width()/2, item->height()/2)).toPoint());
    return triggered.count() == 1;
}
static QByteArray read(const QString &path) {
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly))
        return {};
    return file.readAll();
}
static QFileDialog *pathDialog() {
    for (auto widget : QApplication::topLevelWidgets())
        if (widget->objectName() == "pathDialog")
            if (auto dialog = qobject_cast<QFileDialog *>(widget))
                return dialog;
    return nullptr;
}
static void acceptPathDialog() {
    // Exercise KDE's actual accept path: KFileWidget validates the selection
    // and publishes its URLs before accepting the platform dialog.
    for (auto widget : QApplication::topLevelWidgets()) {
        if (widget->isVisible() && widget->inherits("KDirSelectDialog")) {
            QMetaObject::invokeMethod(widget, "accept");
            return;
        }
        if (!widget->isVisible() || !widget->inherits("KDEPlatformFileDialog"))
            continue;
        for (auto child : widget->findChildren<QWidget *>())
            if (child->inherits("KFileWidget")) {
                QMetaObject::invokeMethod(child, "slotOk");
                return;
            }
    }
    QMetaObject::invokeMethod(pathDialog(), "accept");
}
static void selectDialogPath(const QString &path) {
    auto dialog = pathDialog();
    if (dialog->fileMode() == QFileDialog::Directory) {
        dialog->setDirectory(path);
        // KDE's directory tree selects the requested URL after async loading.
        QTimer::singleShot(200, dialog, []() { acceptPathDialog(); });
        return;
    } else {
        dialog->selectFile(path);
        // QFileDialog intentionally leaves a focused filename edit unchanged
        // on selectFile(). Enter the value as a user would in the fallback UI.
        if (auto input = dialog->findChild<QLineEdit *>("fileNameEdit"))
            input->setText(path);
    }
    QTimer::singleShot(0, dialog, []() { acceptPathDialog(); });
}
static bool answerOverwrite(bool accept) {
    for (auto widget : QApplication::topLevelWidgets()) {
        if (!widget->isVisible() || widget->inherits("QFileDialog") ||
            widget->inherits("KDEPlatformFileDialog") || widget->inherits("KDirSelectDialog"))
            continue;
        for (auto button : widget->findChildren<QAbstractButton *>()) {
            const auto text = QString(button->text()).remove('&');
            if (accept ? (text == "Overwrite" || text == "Yes") : (text == "Cancel" || text == "No")) {
                button->click();
                return true;
            }
        }
    }
    return false;
}
static bool pathDialogHasFilename(const QString &path) {
    // Native KDE reports selected URLs only after validation. Check the visible
    // filename field for the initial Save As suggestion instead.
    for (auto widget : QApplication::topLevelWidgets())
        if (widget->isVisible() && widget->inherits("KDEPlatformFileDialog")) {
            for (auto combo : widget->findChildren<QComboBox *>())
                if (combo->currentText().contains(QFileInfo(path).fileName())) return true;
            return false;
        }
    return pathDialog()->selectedFiles().value(0) == path;
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
            auto tab = findItem(tabs, "tabSelect_" + id + "_" + QString::number(index));
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
    auto menuBar = findItem(window->contentItem(), "mainMenuBar");
    auto actions = menuBar ? menuBar->property("contentItem").value<QQuickItem *>() : nullptr;
    if (!actions)
        return "Missing menu bar";
    if (actions->width() > menuBar->width() + 1)
        return "Menu bar exceeds the window width";
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
        const auto entries = pane.value("tabs").toList();
        for (int index = 0; index < entries.size(); ++index) {
            auto tab = findItem(tabs, "tab_" + id + "_" + QString::number(index));
            if (!tab || !tab->isVisible())
                continue;
            const auto rect = tab->mapRectToItem(tabs, tab->boundingRect());
            if (rect.left() < -1 || rect.top() < -1 || rect.right() > tabs->width() + 1 ||
                rect.bottom() > tabs->height() + 1 || tab->height() + 1 < tab->implicitHeight())
                return "A tab button is clipped by its header";
            const auto editor = entries[index].toMap().value("close_id");
            if (!editor.isNull()) {
                auto close = findItem(tab, "tabClose_" + id + "_" + editor.toString());
                if (!close || !close->isVisible() || close->width() < close->implicitWidth() - 1 ||
                    close->mapRectToItem(tab, close->boundingRect()).right() > tab->width() + 1)
                    return "An editor tab's close button is missing or clipped";
                auto select = findItem(tab, "tabSelect_" + id + "_" + QString::number(index));
                if (!select)
                    return "Missing editor tab selection button";
                const auto closeRect = close->mapRectToItem(select, close->boundingRect());
                if (closeRect.left() < 0 || closeRect.top() < 0 ||
                    closeRect.right() > select->width() || closeRect.bottom() > select->height())
                    return "An editor tab's close button is outside its tab background";
                auto content = select->property("contentItem").value<QQuickItem *>();
                if (!content || content->mapRectToItem(select, content->boundingRect()).right() >
                                    closeRect.left())
                    return "An editor tab's caption overlaps its close button";
                auto background = close->property("background").value<QQuickItem *>();
                if (background && !close->property("hovered").toBool() &&
                    !close->property("down").toBool() &&
                    background->property("color").value<QColor>().alpha() != 0)
                    return "An editor tab's close button has a separate resting background";
            }
        }
        auto grid = findItem(window->contentItem(), "cells_" + id);
        auto browser = findItem(window->contentItem(), "browser_" + id);
        if (browser && browser->isVisible()) {
            for (int row = 0; row < browser->property("count").toInt(); ++row) {
                auto item = findItem(browser, "entry_" + id + "_" + QString::number(row));
                if (!item)
                    continue;
                auto content = item->property("contentItem").value<QQuickItem *>();
                if (content &&
                    (content->y() < 0 || content->y() + content->height() > item->height() + 1))
                    return "File/Git row contents overflow their delegate";
            }
        }
        if (grid && grid->isVisible()) {
            if (grid->y() < chrome->y() + chrome->height() || grid->height() < 0)
                return "Text grid overlaps pane header";
            if (pane.value("rows").toInt() * state->cellHeight() > grid->height())
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

static QString checkGitLayout(Bridge *state, QQuickWindow *window) {
    const auto error = checkLayout(state, window);
    if (!error.isEmpty())
        return error;
    auto browser = findItem(window->contentItem(), "browser_1");
    auto scroll = findItem(window->contentItem(), "gitScroll_1");
    auto view = findItem(window->contentItem(), "gitView_1");
    if (!browser || !scroll || !view ||
        browser->height() < view->property("rowHeight").toInt() + 28)
        return "Git changes list missing or crowded out by controls";
    const auto listRect = browser->mapRectToScene(browser->boundingRect());
    const auto scrollRect = scroll->mapRectToScene(scroll->boundingRect());
    if (listRect.right() > scrollRect.left() ||
        scrollRect.right() > view->mapRectToScene(view->boundingRect()).right())
        return "Git scrollbar gutter overlaps rows or escapes pane";
    for (const auto &name : {"gitRefresh_1", "gitCommit_1", "gitStageAll_1", "gitUnstageAll_1"}) {
        auto button = findItem(window->contentItem(), name);
        if (!button || !button->isVisible())
            return QString("Missing visible Git control: %1").arg(name);
        const auto rect = button->mapRectToItem(view, button->boundingRect());
        if (rect.left() < 0 || rect.right() > view->width() + 1 || rect.top() < 0 ||
            rect.bottom() > view->height())
            return QString("Git control clipped by pane: %1").arg(name);
        if (button->height() < button->implicitHeight() - 1)
            return QString("Git control smaller than its caption: %1").arg(name);
    }
    for (int row = 0; row < browser->property("count").toInt(); ++row) {
        for (const auto &prefix : {"gitStage_1_", "gitDiff_1_"}) {
            auto action = findItem(browser, QString(prefix) + QString::number(row));
            if (!action)
                continue;
            const auto rect = action->mapRectToScene(action->boundingRect());
            if (rect.right() > listRect.right() + 1 || rect.left() < listRect.left() - 1 ||
                rect.intersects(scrollRect))
                return "Git file action overlaps scrollbar or is clipped horizontally";
            auto content = action->property("contentItem").value<QQuickItem *>();
            if (action->height() + 1 < action->implicitHeight() || !content ||
                content->height() < 12)
                return "Git file action clips its contents";
        }
    }
    return {};
}

static void startGitSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0);
    auto rowPath = new QString;
    const auto baseFont = window->property("font").value<QFont>();
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
    auto click = [=](const QString &name) {
        auto item = findItem(window->contentItem(), name);
        if (!item || !item->isVisible() || !item->isEnabled()) {
            finish(false, "Cannot click visible, enabled control: " + name);
            return false;
        }
        QSignalSpy clicked(item, SIGNAL(clicked()));
        QTest::mouseClick(
            window, Qt::LeftButton, Qt::NoModifier,
            item->mapToScene(QPointF(item->width() / 2, item->height() / 2)).toPoint());
        if (clicked.count() != 1) {
            const auto point = item->mapToScene(QPointF(item->width() / 2, item->height() / 2));
            std::function<void(QQuickItem *)> hits = [&](QQuickItem *candidate) {
                if (!candidate->isVisible())
                    return;
                const auto local = candidate->mapFromScene(point);
                if (candidate->contains(local))
                    qWarning() << "Hit" << candidate->metaObject()->className()
                               << candidate->objectName()
                               << candidate->mapRectToScene(candidate->boundingRect());
                else if (candidate->clip())
                    return;
                for (auto child : candidate->childItems())
                    hits(child);
            };
            hits(window->contentItem());
            finish(false, "Mouse click did not reach control: " + name);
            return false;
        }
        return true;
    };
    auto type = [=](const QString &text) {
        for (auto c : text)
            QTest::keyClick(window, c.toLatin1());
    };
    auto check = [=](const QString &name) {
        window->grabWindow().save(dir + "/" + name + ".png");
        const auto error = checkGitLayout(state, window);
        if (!error.isEmpty()) {
            finish(false, name + ": " + error);
            return false;
        }
        return true;
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 150) {
            finish(false, QString("Populated Git timeout at step %1: %2")
                              .arg(*step)
                              .arg(state->frame().value("status").toString()));
            return;
        }
        const auto frame = state->frame();
        if (frame.value("git_busy").toBool())
            return;
        const auto entries = frame.value("git").toList();
        auto browser = findItem(window->contentItem(), "browser_1");
        auto view = findItem(window->contentItem(), "gitView_1");
        switch (*step) {
        case 0:
            if (!frame.value("git_repository").toBool() || entries.size() < 35)
                return;
            state->send({{"action", "focus"}, {"pane", 1}});
            state->command("git");
            state->refresh();
            break;
        case 1:
            if (!check("git-normal"))
                return;
            window->resize(800, 600);
            break;
        case 2:
            if (!check("git-narrow"))
                return;
            window->resize(480, 320);
            break;
        case 3:
            if (!check("git-short"))
                return;
            if (!view->property("compactHeight").toBool()) {
                finish(false, "Short pane did not compact its composer");
                return;
            }
            if (!click("gitCommit_1"))
                return;
            break;
        case 4: {
            auto dialog = view->property("compactComposer").value<QObject *>();
            auto input = findItem(window->contentItem(), "gitDialogMessage_1");
            if (!dialog || !dialog->property("visible").toBool() || !input ||
                dialog->property("height").toReal() > window->height() - 39) {
                finish(false, "Compact commit dialog missing or clipped");
                return;
            }
            input->forceActiveFocus();
            type("Draft survives resizing");
            QTest::keyClick(window, Qt::Key_Escape);
            window->resize(1360, 820);
            QFont largeFont = baseFont;
            largeFont.setPointSize(24);
            window->setProperty("font", largeFont);
            break;
        }
        case 5:
            if (!check("git-large-font"))
                return;
            if (view->property("draft").toString() != "Draft survives resizing") {
                finish(false, "Draft lost during resize");
                return;
            }
            window->setProperty("font", baseFont);
            break;
        case 6: {
            int firstUntracked = 0;
            while (firstUntracked < entries.size() &&
                   !entries[firstUntracked].toMap().value("untracked").toBool())
                ++firstUntracked;
            // Include the preceding row so the inline section header is visible.
            if (!QMetaObject::invokeMethod(browser, "positionViewAtIndex",
                                           Q_ARG(int, firstUntracked - 1), Q_ARG(int, 0))) {
                finish(false, "Could not scroll to untracked section");
                return;
            }
            break;
        }
        case 7:
            if (!check("git-scrolled"))
                return;
            if (!click("gitGroup_1_Untracked"))
                return;
            break;
        case 8: {
            bool working = false;
            for (const auto &value : entries) {
                const auto entry = value.toMap();
                if (entry.value("untracked").toBool()) {
                    finish(false, "Section staging left untracked entries");
                    return;
                }
                working |= !entry.value("staged").toBool();
            }
            if (!working) {
                finish(false, "Section staging unexpectedly staged working changes");
                return;
            }
            QMetaObject::invokeMethod(browser, "positionViewAtBeginning");
            break;
        }
        case 9:
            *rowPath = entries.first().toMap().value("path").toString();
            if (!click("gitStage_1_0"))
                return;
            break;
        case 10: {
            bool unstaged = false;
            for (const auto &value : entries) {
                const auto entry = value.toMap();
                unstaged |= entry.value("path") == *rowPath && !entry.value("staged").toBool();
            }
            if (!unstaged) {
                finish(false, "Row unstage action did not unstage file");
                return;
            }
            if (!click("gitStageAll_1"))
                return;
            break;
        }
        case 11:
            for (const auto &entry : entries) {
                if (!entry.toMap().value("staged").toBool()) {
                    finish(false, "Stage all left working changes");
                    return;
                }
            }
            if (!click("gitUnstageAll_1"))
                return;
            break;
        case 12:
            for (const auto &entry : entries) {
                if (entry.toMap().value("staged").toBool()) {
                    finish(false, "Unstage all left staged changes");
                    return;
                }
            }
            if (!click("gitStageAll_1"))
                return;
            break;
        case 13: {
            // Clicking the composer from another pane must synchronize core
            // focus without moving Qt focus away from the message input.
            state->send({{"action", "focus"}, {"pane", 2}});
            state->refresh();
            QTest::qWait(10);
            auto input = findItem(window->contentItem(), "gitMessage_1");
            input->forceActiveFocus();
            QTest::keyClick(window, Qt::Key_A, Qt::ControlModifier);
            type("GUI bulk commit");
            break;
        }
        case 14:
            if (view->property("draft").toString() != "GUI bulk commit" ||
                frame.value("focus").toInt() != 1) {
                finish(false, "Commit draft did not accept input");
                return;
            }
            if (!click("gitCommit_1"))
                return;
            break;
        case 15:
            if (!frame.value("git_error").toString().contains("Fixture hook rejected") ||
                view->property("draft").toString() != "GUI bulk commit") {
                finish(false, "Failed commit did not retain draft and show error: " +
                                  frame.value("git_error").toString() +
                                  "; draft=" + view->property("draft").toString() +
                                  "; status=" + frame.value("status").toString());
                return;
            }
            if (!check("git-rejected-commit"))
                return;
            QFile::remove(dir + "/workspace/.git/hooks/pre-commit");
            window->resize(480, 320);
            break;
        case 16:
            if (view->property("draft").toString() != "GUI bulk commit" ||
                !view->property("compactHeight").toBool()) {
                finish(false, "Failed commit draft lost when compacting pane");
                return;
            }
            if (!click("gitCommit_1"))
                return;
            break;
        case 17:
            findItem(window->contentItem(), "gitDialogMessage_1")->forceActiveFocus();
            QTest::keyClick(window, Qt::Key_Return, Qt::ControlModifier);
            break;
        case 18:
            if (!frame.value("git_error").toString().isEmpty() || !entries.isEmpty() ||
                !view->property("draft").toString().isEmpty()) {
                finish(false, "Commit retry did not clean repository and clear draft");
                return;
            }
            if (view->property("compactComposer")
                    .value<QObject *>()
                    ->property("visible")
                    .toBool()) {
                // Popup remains visible while its exit animation finishes.
                // The workflow deadline still catches a composer left open.
                return;
            }
            window->resize(1360, 820);
            break;
        case 19:
            if (!check("git-clean"))
                return;
            finish(true, "Populated Git: normal/narrow/short/large fonts, separate scrollbar "
                         "gutter, row/section/bulk staging, compact composer, failed commit draft "
                         "retention and successful retry");
            return;
        }
        ++*step;
    });
    timer->start(220);
}

static void startFeatureSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0), frames = new int(0);
    auto builds = new qulonglong(0);
    auto autoBackground = new QString;
    auto baseline = new QVariantMap;
    auto quietFrames = new int(0);
    QObject::connect(state, &Bridge::frameChanged, state, [frames]() { ++*frames; });
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
    auto key = [=](Qt::Key k, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
        QTest::keyClick(window, k, modifiers);
    };
    auto type = [=](const QString &text) {
        for (auto c : text)
            QTest::keyClick(window, c.toLatin1());
    };
    auto click = [=](QQuickItem *item, bool twice = false) {
        if (!item)
            return false;
        auto point = item->mapToScene(QPointF(item->width() / 2, item->height() / 2)).toPoint();
        if (twice)
            QTest::mouseDClick(window, Qt::LeftButton, Qt::NoModifier, point);
        else
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier, point);
        return true;
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 200) {
            finish(false, QString("Feature timeout at %1: %2")
                              .arg(*step)
                              .arg(state->frame().value("status").toString()));
            return;
        }
        const auto frame = state->frame();
        switch (*step) {
        case 0:
            if (frame.value("git_busy").toBool() || *ticks < 10)
                return;
            if (!frame.value("git_repository").toBool()) {
                finish(false, "Git repository metadata missing");
                return;
            }
            *builds = state->send({{"action", "diagnostics"}}).value("screen_builds").toULongLong();
            *quietFrames = *frames;
            *autoBackground = frame.value("background").toString();
            *baseline = frame;
            break;
        case 1:
            if (*ticks < 30)
                return;
            if (*builds !=
                    state->send({{"action", "diagnostics"}}).value("screen_builds").toULongLong() ||
                *quietFrames != *frames) {
                QStringList changed;
                for (auto entry = frame.cbegin(); entry != frame.cend(); ++entry)
                    if (entry.value() != baseline->value(entry.key()))
                        changed.append(entry.key());
                finish(false, QString("Idle frontend regenerated screens or published frames (%1 → "
                                      "%2 frames; %3 → %4 grids; changed: %5)")
                                  .arg(*quietFrames)
                                  .arg(*frames)
                                  .arg(*builds)
                                  .arg(state->send({{"action", "diagnostics"}})
                                           .value("screen_builds")
                                           .toULongLong())
                                  .arg(changed.join(", ")));
                return;
            }
            key(Qt::Key_F1);
            type("new document");
            break;
        case 2: {
            auto results = findItem(window->contentItem(), "commandResults");
            if (!results || results->property("count").toInt() < 1) {
                finish(false, "Palette did not filter readable actions");
                return;
            }
            key(Qt::Key_Return);
            break;
        }
        case 3:
            key(Qt::Key_F1);
            type("open");
            key(Qt::Key_Return);
            break;
        case 4: {
            auto dialog = pathDialog();
            if (!dialog || !state->pathDialogOpen() || !frame.value("prompt").isNull()) {
                finish(false, "Open action did not show a platform file picker");
                return;
            }
            selectDialogPath(dir + "/workspace/edit.txt");
            break;
        }
        case 5: {
            auto grid = findItem(window->contentItem(), "cells_2");
            if (!grid)
                return;
            grid->forceActiveFocus();
            key(Qt::Key_A, Qt::ControlModifier);
            type("GUI Unicode ");
            QInputMethodEvent preedit("compose", {});
            QCoreApplication::sendEvent(window, &preedit);
            auto cellView = qobject_cast<CellView *>(grid);
            if (!cellView ||
                cellView->inputMethodQuery(Qt::ImSurroundingText).toString() != "GUI Unicode ") {
                finish(false, "IME preedit changed the document before commit");
                return;
            }
            QInputMethodEvent commit;
            commit.setCommitString(QString::fromUtf8("é 日本語 👩‍💻"));
            QCoreApplication::sendEvent(window, &commit);
            key(Qt::Key_S, Qt::ControlModifier);
            break;
        }
        case 6: {
            if (read(dir + "/workspace/edit.txt") !=
                QString::fromUtf8("GUI Unicode é 日本語 👩‍💻").toUtf8())
                return;
            auto grid = qobject_cast<CellView *>(findItem(window->contentItem(), "cells_2"));
            if (!grid) {
                finish(false, "Editor missing after IME commit");
                return;
            }
            key(Qt::Key_Left);
            state->refresh(); // Synchronize the rendered geometry before hit testing.
            const int cursor = grid->inputMethodQuery(Qt::ImCursorPosition).toInt();
            const auto rect = grid->inputMethodQuery(Qt::ImCursorRectangle).toRectF();
            const auto point =
                grid->mapToScene(QPointF(rect.left() + 0.2, rect.center().y())).toPoint();
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier, point);
            if (grid->inputMethodQuery(Qt::ImCursorPosition).toInt() != cursor) {
                finish(false, "Shaped Unicode cursor and mouse hit testing disagree");
                return;
            }
            state->send({{"action", "focus"}, {"pane", 1}});
            state->refresh();
            key(Qt::Key_F1);
            type("git");
            key(Qt::Key_Return);
            break;
        }
        case 7: {
            if (frame.value("git_busy").toBool())
                return;
            const auto entries = frame.value("git").toList();
            int index = -1;
            for (int i = 0; i < entries.size(); ++i)
                if (entries[i].toMap().value("path") == "new file.txt" &&
                    entries[i].toMap().value("untracked").toBool())
                    index = i;
            if (index < 0) {
                finish(false, "Untracked group missing");
                return;
            }
            if (!click(findItem(window->contentItem(), "gitStage_1_" + QString::number(index)))) {
                finish(false, "Direct stage button missing");
                return;
            }
            break;
        }
        case 8: {
            if (frame.value("git_busy").toBool())
                return;
            const auto entries = frame.value("git").toList();
            int index = -1;
            for (int i = 0; i < entries.size(); ++i)
                if (entries[i].toMap().value("path") == "new file.txt" &&
                    entries[i].toMap().value("staged").toBool())
                    index = i;
            if (index < 0)
                return;
            if (!click(findItem(window->contentItem(), "entry_1_" + QString::number(index)),
                       true)) {
                finish(false, "Staged file delegate missing");
                return;
            }
            break;
        }
        case 9: {
            if (frame.value("git_busy").toBool())
                return;
            bool readonly = false;
            for (const auto &p : frame.value("panes").toList())
                if (p.toMap().value("focused").toBool())
                    readonly = p.toMap().value("read_only").toBool();
            if (!readonly) {
                finish(false, "Git diff was not a read-only inspection");
                return;
            }
            auto grid = findItem(window->contentItem(), "cells_" + frame.value("focus").toString());
            grid->forceActiveFocus();
            type("bad");
            if (!state->frame().value("status").toString().contains("read-only") ||
                state->frame().value("dirty").toBool()) {
                finish(false, "Diff mutation or dirty-state regression");
                return;
            }
            state->command("close");
            break;
        }
        case 10: {
            auto input = findItem(window->contentItem(), "gitMessage_1");
            if (!input) {
                finish(false, "Inline commit message missing");
                return;
            }
            input->forceActiveFocus();
            type("GUI smoke commit");
            break;
        }
        case 11: {
            auto button = findItem(window->contentItem(), "gitCommit_1");
            if (!button || !button->isEnabled() || !click(button)) {
                finish(false, "Inline commit control unavailable");
                return;
            }
            break;
        }
        case 12:
            if (frame.value("git_busy").toBool())
                return;
            if (!frame.value("git_error").toString().isEmpty()) {
                finish(false, "Git commit failed: " + frame.value("git_error").toString());
                return;
            }
            key(Qt::Key_F1);
            type("settings");
            key(Qt::Key_Return);
            break;
        case 13: {
            auto dialog = window->findChild<QObject *>("settingsDialog");
            if (!dialog || !dialog->property("visible").toBool()) {
                finish(false, "Settings action did not open GUI controls");
                return;
            }
            auto indent = findItem(window->contentItem(), "settingsIndent");
            if (!indent) {
                finish(false, "Indent setting missing");
                return;
            }
            indent->forceActiveFocus();
            key(Qt::Key_Up);
            break;
        }
        case 14:
            if (frame.value("background").toString() != *autoBackground) {
                finish(false, "Changing indentation reset the automatic desktop palette");
                return;
            }
            if (!read(dir + "/config/slate/settings.toml").contains("indent_width = 5")) {
                finish(false, "Settings control did not persist its change");
                return;
            }
            key(Qt::Key_Escape);
            if (const auto error = checkLayout(state, window); !error.isEmpty()) {
                finish(false, error);
                return;
            }
            finish(true, "Idle wakeups, searchable palette, argument forms, Unicode IME, Git "
                         "stage/diff/commit and settings controls");
            return;
        }
        ++*step;
    });
    timer->start(100);
}

static void startStartupSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    const bool expected = qEnvironmentVariable("SLATE_GUI_EXPECT_EDITOR_ONLY") == "1";
    auto timer = new QTimer(state);
    auto step = new int(0);
    auto ticks = new int(0);
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json");
        report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}}).toJson());
        window->grabWindow().save(dir + "/gui.png");
        timer->stop();
        QGuiApplication::exit(pass ? 0 : 2);
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        state->refresh();
        if (++*ticks > 80) {
            finish(false, "Startup smoke timeout at step " + QString::number(*step));
            return;
        }
        const auto frame = state->frame();
        switch (*step) {
        case 0:
            if (frame.value("editor_only").toBool() != expected || state->paneIds().size() != (expected ? 1 : 3)) {
                finish(false, "Wrong startup mode or visible pane count");
                return;
            }
            window->grabWindow().save(dir + "/startup.png");
            if (expected && QFile::exists(dir + "/shell-starts")) {
                finish(false, "Editor-only startup launched a hidden shell");
                return;
            }
            if (!clickMenuAction(window, timer, "View", "toggle-workspace")) {
                finish(false, "Workspace toggle menu action unavailable");
                return;
            }
            break;
        case 1:
            if (frame.value("editor_only").toBool() == expected) {
                finish(false, "Workspace button did not toggle the layout");
                return;
            }
            QTest::keyClick(window, Qt::Key_F10);
            break;
        case 2:
            if (frame.value("editor_only").toBool() != expected) {
                finish(false, "F10 did not restore startup layout");
                return;
            }
            QTest::keyClick(window, Qt::Key_Comma, Qt::ControlModifier);
            break;
        case 3: {
            auto dialog = window->findChild<QObject *>("settingsDialog");
            auto setting = findItem(window->contentItem(), "settingsFileStartup");
            if (!dialog || !dialog->property("visible").toBool() || !setting) {
                finish(false, QString("Settings shortcut: dialog=%1, control=%2, prompt=%3, focus=%4")
                    .arg(dialog && dialog->property("visible").toBool()).arg(bool(setting))
                    .arg(frame.value("prompt").toMap().value("kind").toString())
                    .arg(window->activeFocusItem() ? window->activeFocusItem()->objectName() : "none"));
                return;
            }
            setting->forceActiveFocus();
            QTest::keyClick(window, Qt::Key_End);
            break;
        }
        case 4: {
            if (!read(dir + "/config/slate/settings.toml").contains("file_startup = \"workspace\"")) {
                finish(false, "File startup control did not save its setting");
                return;
            }
            auto setting = findItem(window->contentItem(), "settingsDirectoryStartup");
            if (!setting) {
                finish(false, "Missing directory startup control");
                return;
            }
            setting->forceActiveFocus();
            QTest::keyClick(window, Qt::Key_Home);
            break;
        }
        case 5:
            if (!read(dir + "/config/slate/settings.toml").contains("directory_startup = \"editor-only\"")) {
                finish(false, "Directory startup control did not save its setting");
                return;
            }
            if (frame.value("editor_only").toBool() != expected) {
                finish(false, "Startup setting changed the current layout");
                return;
            }
            window->grabWindow().save(dir + "/settings.png");
            QTest::keyClick(window, Qt::Key_Escape);
            window->resize(480, 320);
            break;
        case 6:
            if (const auto error = checkLayout(state, window); !error.isEmpty()) {
                finish(false, error);
                return;
            }
            window->grabWindow().save(dir + "/compact.png");
            finish(true, "Startup mode, lazy shells, workspace button/F10, settings shortcut and persisted startup controls");
            return;
        }
        ++*step;
    });
    timer->start(150);
}

static void startFileDialogSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    const QString picked = dir + QString::fromUtf8("/workspace/picked 猫 #% .txt");
    const QString saved = dir + QString::fromUtf8("/workspace/saved 猫 #% .txt");
    const QString scratch = dir + "/workspace/scratch.txt";
    const QString folder = dir + QString::fromUtf8("/workspace/folder 猫 #%");
    QDir().mkpath(folder);
    QFile fixture(picked);
    fixture.open(QIODevice::WriteOnly);
    fixture.write("picked document");
    fixture.close();
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0);
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json");
        report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}, {"steps", *step}}).toJson());
        timer->stop();
        QGuiApplication::exit(pass ? 0 : 2);
    };
    auto invoke = [=](const QString &id) {
        QMetaObject::invokeMethod(window, "invokeAction", Q_ARG(QVariant, QVariant(id)));
    };
    auto checkDialog = [=](QFileDialog::FileMode mode, QFileDialog::AcceptMode accept) {
        auto dialog = pathDialog();
        if (!dialog || !state->pathDialogOpen() || !state->frame().value("prompt").isNull() ||
            dialog->fileMode() != mode || dialog->acceptMode() != accept ||
            dialog->testOption(QFileDialog::DontUseNativeDialog) ||
            dialog->testOption(QFileDialog::DontConfirmOverwrite) ||
            dialog->supportedSchemes() != QStringList{"file"} ||
            dialog->windowHandle()->transientParent() != window) {
            finish(false, "Platform dialog mode, parenting, prompt routing or defaults incorrect");
            return false;
        }
        if (!qEnvironmentVariableIsEmpty("SLATE_GUI_REQUIRE_KDE_DIALOGS")) {
            bool native = false;
            for (auto widget : QApplication::topLevelWidgets())
                if (widget->isVisible()) {
                    if (widget->inherits("KDEPlatformFileDialog") || widget->inherits("KDirSelectDialog"))
                        native = true;
                }
            if (!native) {
                finish(false, "KDE's native file dialog was not used");
                return false;
            }
        }
        return true;
    };
    auto select = [](const QString &path) {
        selectDialogPath(path);
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 200) {
            finish(false, QString("File dialog timeout at %1: %2").arg(*step).arg(state->frame().value("status").toString()));
            return;
        }
        const auto frame = state->frame();
        switch (*step) {
        case 0: {
            if (!clickMenuAction(window, timer, "File", "open")) {
                finish(false, "File > Open File menu action unavailable");
                return;
            }
            break;
        }
        case 1:
            if (!checkDialog(QFileDialog::ExistingFile, QFileDialog::AcceptOpen)) return;
            pathDialog()->reject();
            break;
        case 2:
            if (state->pathDialogOpen() || !frame.value("prompt").isNull()) {
                finish(false, "Cancel left a path prompt or dialog open");
                return;
            }
            QTest::keyClick(window, Qt::Key_O, Qt::ControlModifier);
            break;
        case 3:
            if (!checkDialog(QFileDialog::ExistingFile, QFileDialog::AcceptOpen)) return;
            select(picked);
            break;
        case 4:
            if (state->send({{"action", "file_dialog_context"}}).value("path") != picked) return;
            if (!clickMenuAction(window, timer, "File", "save-as")) {
                finish(false, "File > Save As menu action unavailable"); return;
            }
            break;
        case 5:
            if (!checkDialog(QFileDialog::AnyFile, QFileDialog::AcceptSave)) return;
            if (!pathDialogHasFilename(picked)) {
                finish(false, "Save As did not preselect the current document");
                return;
            }
            select(saved);
            break;
        case 6:
            if (read(saved) != "picked document") return;
            state->send({{"action", "new"}});
            state->send({{"action", "paste"}, {"text", "scratch document"}});
            state->refresh();
            QTest::keyClick(window, Qt::Key_S, Qt::ControlModifier);
            break;
        case 7:
            if (!checkDialog(QFileDialog::AnyFile, QFileDialog::AcceptSave)) return;
            pathDialog()->reject();
            break;
        case 8:
            if (QFile::exists(scratch) || !frame.value("dirty").toBool() || state->pathDialogOpen()) {
                finish(false, "Cancelling an untitled save changed the document");
                return;
            }
            QTest::keyClick(window, Qt::Key_S, Qt::ControlModifier | Qt::ShiftModifier);
            break;
        case 9:
            if (!checkDialog(QFileDialog::AnyFile, QFileDialog::AcceptSave)) return;
            select(scratch);
            break;
        case 10:
            if (read(scratch) != "scratch document") return;
            invoke("open-folder");
            break;
        case 11:
            if (!checkDialog(QFileDialog::Directory, QFileDialog::AcceptOpen)) return;
            select(folder);
            break;
        case 12:
            if (frame.value("browser") != folder) return;
            if (state->send({{"action", "file_dialog_context"}}).value("path") != scratch) {
                finish(false, "Opening a folder replaced the editor document");
                return;
            }
            QTest::keyClick(window, Qt::Key_O, Qt::ControlModifier | Qt::ShiftModifier);
            break;
        case 13:
            if (!checkDialog(QFileDialog::Directory, QFileDialog::AcceptOpen)) return;
            pathDialog()->reject();
            break;
        case 14:
            invoke("save-as");
            break;
        case 15:
            if (!checkDialog(QFileDialog::AnyFile, QFileDialog::AcceptSave)) return;
            select(saved);
            break;
        case 16:
            if (!answerOverwrite(false)) return;
            if (read(saved) != "picked document") {
                finish(false, "Cancelled overwrite modified the existing file");
                return;
            }
            pathDialog()->reject();
            break;
        case 17:
            invoke("save-as");
            break;
        case 18:
            if (!checkDialog(QFileDialog::AnyFile, QFileDialog::AcceptSave)) return;
            select(saved);
            break;
        case 19:
            if (!answerOverwrite(true)) return;
            break;
        case 20:
            if (read(saved) != "scratch document") return;
            finish(true, "Platform file/folder/save dialogs, File menu, palette routing, shortcuts, cancellation, untitled saves, Unicode paths and confirmed/cancelled overwrites");
            return;
        }
        ++*step;
    });
    timer->start(150);
}

static void startTabCloseSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0);
    auto target = new QString;
    auto tabs = [=](int pane = 2) {
        for (const auto &value : state->frame().value("panes").toList())
            if (value.toMap().value("id").toInt() == pane)
                return value.toMap().value("tabs").toList();
        return QVariantList{};
    };
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json"); report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}, {"steps", *step}}).toJson());
        window->grabWindow().save(dir + "/gui.png");
        timer->stop(); QGuiApplication::exit(pass ? 0 : 2);
    };
    auto clickClose = [=](const QString &editor, int pane = 2) {
        auto button = findItem(window->contentItem(), "tabClose_" + QString::number(pane) + "_" + editor);
        if (!button || !button->isVisible()) {
                finish(false, "Visible file or terminal tab has no close button"); return false;
        }
        QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier,
            button->mapToScene(QPointF(button->width()/2, button->height()/2)).toPoint());
        return true;
    };
    auto answer = [=](QMessageBox::StandardButton choice) {
        for (auto widget : QApplication::topLevelWidgets())
            if (auto dialog = qobject_cast<QMessageBox *>(widget))
                if (dialog->objectName() == "closeTabDialog") {
                    if (dialog->defaultButton() != dialog->button(QMessageBox::Cancel) ||
                        dialog->textFormat() != Qt::PlainText ||
                        dialog->windowHandle()->transientParent() != window) {
                        finish(false, "Close confirmation defaults or parent are incorrect"); return false;
                    }
                    dialog->button(choice)->click(); return true;
                }
        finish(false, "Unsaved close did not show a standard confirmation dialog"); return false;
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 150) { finish(false, QString("Tab close timeout at %1: %2").arg(*step).arg(state->frame().value("status").toString())); return; }
        const auto entries = tabs();
        switch (*step) {
        case 0:
            state->send({{"action", "open"}, {"path", dir + "/workspace/edit.txt"}}); state->refresh(); break;
        case 1:
            if (state->send({{"action", "file_dialog_context"}}).value("path") != dir + "/workspace/edit.txt") return;
            state->send({{"action", "close_tab"}, {"pane", 2}, {"view", 11}});
            state->send({{"action", "open"}, {"path", dir + "/workspace/second.txt"}}); state->refresh(); break;
        case 2:
            if (entries.size() != 2) return;
            *target = entries.last().toMap().value("editor_id").toString();
            if (!clickClose(entries.first().toMap().value("editor_id").toString())) return;
            break;
        case 3:
            if (entries.size() != 1 || entries.first().toMap().value("editor_id").toString() != *target ||
                state->send({{"action", "file_dialog_context"}}).value("path") != dir + "/workspace/second.txt") {
                finish(false, "Closing a background tab changed the active document"); return;
            }
            state->send({{"action", "paste"}, {"text", "EDIT"}}); state->refresh();
            if (!clickClose(*target)) return;
            break;
        case 4:
            if (!state->closeDialogOpen() || !answer(QMessageBox::Cancel)) return;
            break;
        case 5:
            if (!state->frame().value("dirty").toBool() || entries.size() != 1 || state->closeDialogOpen()) {
                finish(false, "Cancelling close lost the edited tab"); return;
            }
            if (!clickClose(*target)) return;
            break;
        case 6:
            if (!state->closeDialogOpen() || !answer(QMessageBox::Discard)) return;
            break;
        case 7:
            if (entries.size() != 1 || entries.first().toMap().value("editor_id").toString() == *target ||
                state->frame().value("dirty").toBool() || read(dir + "/workspace/second.txt") != "second original") {
                finish(false, "Discard did not remove the selected tab and leave an empty editor"); return;
            }
            for (int i = 0; i < 6; ++i) state->send({{"action", "new"}});
            state->refresh(); window->resize(480, 320); break;
        case 8: {
            const auto error = checkLayout(state, window);
            if (!error.isEmpty()) { finish(false, error); return; }
            *target = entries.last().toMap().value("editor_id").toString();
            if (!clickClose(*target)) return;
            break;
        }
        case 9:
            if (entries.size() != 6) { finish(false, "Crowded tab close removed the wrong tab"); return; }
            QTest::keyClick(window, Qt::Key_W, Qt::ControlModifier); break;
        case 10:
            if (entries.size() != 5) { finish(false, "Ctrl+W did not close the active tab"); return; }
            state->send({{"action", "preset"}, {"name", "bottom_terminal"}});
            state->send({{"action", "focus"}, {"pane", 3}});
            state->refresh(); window->resize(1360, 820); break;
        case 11:
        case 12:
            if (!clickMenuAction(window, timer, "Terminal", "terminal")) {
                finish(false, "Terminal > New Terminal menu action unavailable"); return;
            }
            break;
        case 13: {
            const auto terminals = tabs(3);
            if (terminals.size() != 3) { finish(false, "New Terminal menu did not create terminal tabs"); return; }
            const auto error = checkLayout(state, window);
            if (!error.isEmpty()) { finish(false, error); return; }
            window->grabWindow().save(dir + "/terminal-tabs.png");
            *target = terminals.last().toMap().value("close_id").toString();
            if (!clickClose(terminals.first().toMap().value("close_id").toString(), 3)) return;
            break;
        }
        case 14: {
            const auto terminals = tabs(3);
            if (terminals.size() != 2 || !terminals.last().toMap().value("active").toBool() ||
                terminals.last().toMap().value("close_id").toString() != *target ||
                state->frame().value("focus").toInt() != 3 || state->closeDialogOpen()) {
                finish(false, "Closing an inactive terminal changed focus or the active terminal"); return;
            }
            if (!clickClose(*target, 3)) return;
            break;
        }
        case 15: {
            const auto terminals = tabs(3);
            if (terminals.size() != 1 || terminals.first().toMap().value("close_id").isNull()) {
                finish(false, "Closing the active terminal removed the wrong session"); return;
            }
            if (!clickMenuAction(window, timer, "File", "close")) {
                finish(false, "File > Close Tab is unavailable for a terminal"); return;
            }
            break;
        }
        case 16: {
            const auto terminals = tabs(3);
            if (terminals.size() != 1 || !terminals.first().toMap().value("close_id").isNull() ||
                state->frame().value("status").toString().startsWith("Error")) {
                finish(false, "The final terminal did not close cleanly"); return;
            }
            if (!openMenu(window, timer, "File")) { finish(false, "File menu did not open"); return; }
            break;
        }
        case 17: {
            auto save = findMenuItem(window, "menuAction_save");
            auto close = findMenuItem(window, "menuAction_close");
            if (!save || !close || save->property("enabled").toBool() || close->property("enabled").toBool()) {
                finish(false, QString("File menu availability: save=%1 close=%2 focus=%3")
                    .arg(save ? (save->property("enabled").toBool() ? "enabled" : "disabled") : "missing")
                    .arg(close ? (close->property("enabled").toBool() ? "enabled" : "disabled") : "missing")
                    .arg(state->frame().value("focus").toInt())); return;
            }
            window->grabWindow().save(dir + "/file-menu.png");
            dismissMenu(window);
            state->send({{"action", "focus"}, {"pane", 2}});
            state->send({{"action", "paste"}, {"text", "MENU_EDIT"}}); state->refresh();
            break;
        }
        case 18:
            if (!clickMenuAction(window, timer, "Edit", "select-all")) {
                finish(false, "Edit > Select All menu action unavailable"); return;
            }
            break;
        case 19:
            if (!clickMenuAction(window, timer, "Edit", "cut")) {
                finish(false, "Edit > Cut menu action unavailable"); return;
            }
            break;
        case 20:
            if (QGuiApplication::clipboard()->text() != "MENU_EDIT") {
                const auto editor = state->send({{"action", "input_context"}, {"pane", 2}}).value("editor").toMap();
                finish(false, QString("Edit > Cut clipboard mismatch: remaining=%1 selection=%2")
                    .arg(editor.value("surrounding").toString().size())
                    .arg(editor.value("selection").toString().size())); return;
            }
            if (!clickMenuAction(window, timer, "Edit", "paste")) {
                finish(false, "Edit > Paste menu action unavailable"); return;
            }
            break;
        case 21: {
            if (!openMenu(window, timer, "Edit")) { finish(false, "Edit menu did not open"); return; }
            break;
        }
        case 22: {
            auto undo = findMenuItem(window, "menuAction_undo");
            if (!undo || !undo->property("enabled").toBool()) { finish(false, "Edit menu did not enable editor actions"); return; }
            window->grabWindow().save(dir + "/edit-menu.png");
            dismissMenu(window);
            break;
        }
        case 23:
            if (!clickMenuAction(window, timer, "File", "quit")) {
                finish(false, "File > Quit menu action unavailable"); return;
            }
            break;
        case 24: {
            auto dialog = window->findChild<QObject *>("quitDialog");
            if (!dialog || !dialog->property("visible").toBool() || !state->frame().value("dirty").toBool()) {
                finish(false, "File > Quit did not confirm unsaved changes"); return;
            }
            QMetaObject::invokeMethod(dialog, "reject");
            break;
        }
        case 25: {
            auto dialog = window->findChild<QObject *>("quitDialog");
            if (!dialog || dialog->property("visible").toBool() || !state->frame().value("dirty").toBool() ||
                state->frame().value("quit").toBool()) {
                finish(false, "Cancelling File > Quit lost unsaved work"); return;
            }
            finish(true, "GUI file/terminal close buttons, inactive-tab focus, native unsaved/cancel/discard confirmation, final tabs, crowded headers, Ctrl+W, menu availability, Edit clipboard actions and Quit cancellation"); return;
        }
        }
        ++*step;
    });
    timer->start(150);
}

void startPerformanceSmoke(Bridge *, QQuickWindow *);
void startSmoke(Bridge *state, QQuickWindow *window) {
    if (QGuiApplication::desktopFileName() != "slate" ||
        QGuiApplication::windowIcon().pixmap(64, 64).isNull()) {
        qWarning("Application desktop identity or embedded SVG icon is missing");
        QGuiApplication::exit(2);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_PERF_SMOKE")) {
        startPerformanceSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_FILE_DIALOG_SMOKE")) {
        startFileDialogSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_TAB_CLOSE_SMOKE")) {
        startTabCloseSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_STARTUP_SMOKE")) {
        startStartupSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_GIT_SMOKE")) {
        startGitSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_FEATURE_SMOKE")) {
        startFeatureSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_LAYOUT_SMOKE")) {
        startLayoutSmoke(state, window);
        return;
    }
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto step = new int(0);
    auto ticks = new int(0);
    auto traceStep = new int(-1);
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
        type(":" + text);
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
        if (!qEnvironmentVariableIsEmpty("SLATE_GUI_SMOKE_TRACE") && *traceStep != *step) {
            *traceStep = *step;
            qInfo() << "Smoke step" << *step << state->frame().value("status");
        }
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
            if (state->files()->rowCount() < 3)
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
            type("G");
            state->refresh(); // Flush the dirty-title transition before retaining the tab.
            QPointer<QQuickItem> editingTab;
            for (const auto &value : state->frame().value("panes").toList()) {
                const auto pane = value.toMap();
                if (pane.value("id").toInt() != 2)
                    continue;
                const auto tabs = pane.value("tabs").toList();
                for (int index = 0; index < tabs.size(); ++index)
                    if (tabs[index].toMap().value("active").toBool())
                        editingTab =
                            findItem(window->contentItem(), "tab_2_" + QString::number(index));
            }
            type("UI edited");
            key(Qt::Key_Return);
            type("second line");
            if (!editingTab) {
                finish(false, "Typing recreated unchanged tab controls");
                return;
            }
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
                finish(false,
                       "New terminal did not become active: " + frame.value("status").toString() +
                           "; focus=" + frame.value("focus").toString() + "; input=" +
                           (findItem(window->contentItem(), "commandSearch")
                                ? findItem(window->contentItem(), "commandSearch")
                                      ->property("text")
                                      .toString()
                                : "missing"));
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
            auto files = state->frame().value("files").toList();
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
