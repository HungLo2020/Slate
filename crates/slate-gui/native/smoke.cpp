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
#include <QAccessible>
#include <QMimeData>
#include <QTextCharFormat>
#include <QWheelEvent>
#include <functional>

static QQuickItem *findItem(QQuickItem *item, const QString &name) {
    if (item->objectName() == name)
        return item;
    for (auto child : item->childItems())
        if (auto found = findItem(child, name))
            return found;
    return nullptr;
}
static QQuickItem *findVisibleItem(QQuickItem *item, const QString &name) {
    if (item->objectName() == name && item->isVisible())
        return item;
    for (auto child : item->childItems())
        if (auto found = findVisibleItem(child, name))
            return found;
    return nullptr;
}
static QQuickItem *findMenuItem(QQuickWindow *window, const QString &name) {
    // Pane context menus reuse the menu-bar object names; prefer the open one.
    if (auto visible = findVisibleItem(window->contentItem(), name))
        return visible;
    auto item = findItem(window->contentItem(), name);
    // Desktop styles can place menus in separate popup windows.
    for (auto candidate : QGuiApplication::allWindows())
        if (auto popup = qobject_cast<QQuickWindow *>(candidate))
            if (popup != window && popup->isVisible())
                if (auto found = findVisibleItem(popup->contentItem(), name))
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
    const QStringList names{"fileMenu", "editMenu", "viewMenu", "goMenu", "runMenu"};
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
    // Let the opened menu finish its first frame; real pointers never click
    // within the same event-loop turn that shows a popup.
    QTest::qWait(60);
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
    // An untrusted repository asks once before Git runs; 1 = dialog seen, 2 = trusted.
    auto trust = new int(0);
    auto shownGit = new bool(false);
    auto settling = new int(0);
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
        const auto error = checkGitLayout(state, window);
        // Resizes and font changes can take a few frames to lay out.
        if (!error.isEmpty() && ++*settling < 8)
            return false;
        *settling = 0;
        window->grabWindow().save(dir + "/" + name + ".png");
        if (!error.isEmpty()) {
            finish(false, name + ": " + error);
            return false;
        }
        return true;
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 150) {
            const auto frame = state->frame();
            finish(false, QString("Populated Git timeout at step %1: %2 (repository=%3 restricted=%4 entries=%5 error=%6)")
                              .arg(*step)
                              .arg(frame.value("status").toString())
                              .arg(frame.value("git_repository").toBool())
                              .arg(frame.value("git_restricted").toBool())
                              .arg(frame.value("git").toList().size())
                              .arg(frame.value("git_error").toString()));
            return;
        }
        const auto frame = state->frame();
        if (frame.value("git_busy").toBool())
            return;
        if (frame.value("prompt").toMap().value("kind") == "trust") {
            auto dialog = window->findChild<QObject *>("choiceDialog");
            if (!dialog || !dialog->property("visible").toBool())
                return;
            if (*trust == 0) {
                // Click once the dialog has been laid out and painted.
                *trust = 1;
                return;
            }
            QQuickItem *accept = nullptr;
            for (auto candidate : window->contentItem()->findChildren<QQuickItem *>())
                if (candidate->objectName() == "choiceAccept" && candidate->isVisible())
                    accept = candidate;
            if (!accept) {
                finish(false, "Missing Trust Folder button");
                return;
            }
            window->grabWindow().save(dir + "/git-trust.png");
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier,
                              accept->mapToScene(QPointF(accept->width() / 2,
                                                         accept->height() / 2)).toPoint());
            *trust = 2;
            return;
        }
        const auto entries = frame.value("git").toList();
        auto browser = findItem(window->contentItem(), "browser_1");
        auto view = findItem(window->contentItem(), "gitView_1");
        switch (*step) {
        case 0:
            if (!frame.value("git_repository").toBool())
                return;
            // An untrusted repository shows no changes until it is trusted.
            if (frame.value("git_restricted").toBool()) {
                if (!entries.isEmpty()) {
                    finish(false, "Git ran in an untrusted repository");
                    return;
                }
                if (*trust != 0)
                    return;
                auto button = findItem(window->contentItem(), "gitTrust_1");
                if (button && button->isVisible()) {
                    window->grabWindow().save(dir + "/git-restricted.png");
                    click("gitTrust_1");
                } else if (!*shownGit) {
                    *shownGit = true;
                    state->send({{"action", "focus"}, {"pane", 1}});
                    state->command("git");
                    state->refresh();
                }
                return;
            }
            if (entries.size() < 35)
                return;
            if (!*shownGit) {
                state->send({{"action", "focus"}, {"pane", 1}});
                state->command("git");
                state->refresh();
            }
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
            if (!view->property("compactHeight").toBool() && ++*settling < 8)
                return;
            *settling = 0;
            if (view->property("draft").toString() != "GUI bulk commit" ||
                !view->property("compactHeight").toBool()) {
                finish(false, "Failed commit did not settle into compact layout; draft=" +
                              view->property("draft").toString() +
                              "; height=" + QString::number(view->height()));
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
            if (*trust != 2) {
                finish(false, "An untrusted repository did not ask for trust");
                return;
            }
            finish(true, "Populated Git: trust prompt, normal/narrow/short/large fonts, separate scrollbar "
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
            if (frame.value("git_busy").toBool() || *ticks < 20)
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
            if (*ticks < 40)
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
    const QString requests = dir + "/workspace-requests.log";
    qputenv("SLATE_GUI_NEW_WINDOW_LOG", requests.toUtf8());
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
        QMetaObject::invokeMethod(window, "invokeAction", Q_ARG(QVariant, QVariant(id)),
            Q_ARG(QVariant, QVariant("")), Q_ARG(QVariant, QVariant(0)), Q_ARG(QVariant, QVariant(-1)));
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
            if (!checkDialog(QFileDialog::ExistingFiles, QFileDialog::AcceptOpen)) return;
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
            if (!checkDialog(QFileDialog::ExistingFiles, QFileDialog::AcceptOpen)) return;
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
            if (!read(requests).contains(("open-workspace:" + folder).toUtf8())) return;
            if (frame.value("browser") != dir + "/workspace") { finish(false, "Opening a workspace changed the existing window root"); return; }
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

static void startCommandSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0);
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json"); report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}, {"steps", *step}}).toJson());
        window->grabWindow().save(dir + "/gui.png");
        timer->stop(); QGuiApplication::exit(pass ? 0 : 2);
    };
    auto palette = [=](const QString &command) {
        QTest::keyClick(window, Qt::Key_F1);
        auto input = findItem(window->contentItem(), "commandSearch");
        if (!input || !input->isVisible()) return false;
        input->setProperty("text", command);
        QTest::keyClick(window, Qt::Key_Return);
        return true;
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 150) { finish(false, QString("Command routing timeout at %1").arg(*step)); return; }
        if (*step == 0) {
            state->send({{"action", "paste"}, {"text", "DIRTY"}}); state->refresh();
            ++*step; return;
        }
        const int scenario = (*step - 1) / 2;
        const bool trigger = (*step - 1) % 2 == 0;
        if (scenario < 14) {
            if (trigger) {
                state->send({{"action", "focus"}, {"pane", 2}}); state->refresh();
                auto cells = findItem(window->contentItem(), "cells_2");
                if (!cells) { finish(false, "Editor missing"); return; }
                cells->forceActiveFocus();
                switch (scenario) {
                case 0: QTest::keyClick(window, Qt::Key_Q, Qt::ControlModifier); break;
                case 1: QTest::keyClick(window, Qt::Key_F5); break;
                case 2: if (!palette("quit")) { finish(false, "Palette missing"); return; } break;
                case 3: if (!palette(":quit")) { finish(false, "Raw palette missing"); return; } break;
                case 4: window->close(); break;
                case 5: if (!clickMenuAction(window, timer, "File", "quit")) { finish(false, "File Quit unavailable"); return; } break;
                case 6: if (!palette(":discard-quit")) { finish(false, "Discard palette missing"); return; } break;
                case 7: QTest::keyClick(window, Qt::Key_F4); break;
                case 8: if (!clickMenuAction(window, timer, "File", "settings")) { finish(false, "Settings menu unavailable"); return; } break;
                case 9: QTest::keyClick(window, Qt::Key_F2); break;
                case 10: if (!palette(":settings")) { finish(false, "Settings palette missing"); return; } break;
                case 11: {
                    state->send({{"action", "focus"}, {"pane", 1}});
                    state->send({{"action", "add_view"}, {"kind", "git"}}); state->refresh();
                    auto message = findItem(window->contentItem(), "gitMessage_1");
                    if (!message) { finish(false, "Git message field missing"); return; }
                    message->forceActiveFocus();
                    QTest::keyClick(window, Qt::Key_Q, Qt::ControlModifier);
                    break;
                }
                case 12: if (!palette(":layout-save")) { finish(false, "Layout palette missing"); return; } break;
                case 13: QTest::keyClick(window, Qt::Key_F3); break;
                }
            } else {
                const auto name = scenario >= 12 ? "editPrompt" : scenario >= 8 && scenario <= 10 ? "settingsDialog" :
                    scenario == 6 || scenario == 7 ? "confirmDiscard" : "quitDialog";
                auto dialog = window->findChild<QObject *>(name);
                if (!dialog || !dialog->property("visible").toBool() || !state->frame().value("dirty").toBool()) {
                    finish(false, QString("Route %1 did not open %2 while preserving edits").arg(scenario).arg(name)); return;
                }
                QMetaObject::invokeMethod(dialog, "reject");
                state->refresh();
                if (!window->isVisible() || state->frame().value("quit").toBool() || !state->frame().value("dirty").toBool()) {
                    finish(false, "Cancelling a command lost edits or closed the window"); return;
                }
            }
        } else if (scenario == 14) {
            if (trigger) {
                if (state->frame().value("git_busy").toBool() || state->frame().value("git").toList().isEmpty()) return;
                state->send({{"action", "focus"}, {"pane", 1}}); state->refresh();
                auto button = findItem(window->contentItem(), "paneActions_1");
                if (!button || !button->isVisible()) { finish(false, "Git pane menu button missing"); return; }
                QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier,
                    button->mapToScene(QPointF(button->width()/2, button->height()/2)).toPoint());
            } else {
                const auto rows = state->frame().value("git").toList();
                const auto paneInfo = state->commandInfo("stage", 1);
                auto stage = findMenuItem(window, "menuAction_stage");
                auto unstage = findMenuItem(window, "menuAction_unstage");
                auto diff = findMenuItem(window, "menuAction_diff");
                if (!stage || !unstage || !diff || stage->isEnabled() != paneInfo.value("enabled").toBool() ||
                    unstage->isEnabled() != state->commandInfo("unstage", 1).value("enabled").toBool() || !diff->isEnabled()) {
                    finish(false, "Git pane menu disagrees with the shared catalog"); return;
                }
                for (int row = 0; row < rows.size(); ++row) {
                    const bool staged = rows[row].toMap().value("staged").toBool();
                    const auto info = state->commandInfo(staged ? "unstage" : "stage", 1, row);
                    auto button = findItem(window->contentItem(), "gitStage_1_" + QString::number(row));
                    if (!button || !info.value("enabled").toBool() || !button->isEnabled() ||
                        state->commandInfo(staged ? "stage" : "unstage", 1, row).value("enabled").toBool()) {
                        finish(false, QString("Git row %1 availability: staged=%2 button=%3 enabled=%4 info=%5 opposite=%6")
                            .arg(row).arg(staged).arg(button != nullptr).arg(button && button->isEnabled())
                            .arg(info.value("enabled").toBool())
                            .arg(state->commandInfo(staged ? "stage" : "unstage", 1, row).value("enabled").toBool())); return;
                    }
                }
                QTest::keyClick(window, Qt::Key_Escape);
                state->send({{"action", "focus"}, {"pane", 2}}); state->refresh();
                const auto before = state->diagnostics().value("catalog_requests").toULongLong();
                state->commandInfo("stage", 1, 0); state->commandInfo("unstage", 1, 0);
                if (state->diagnostics().value("catalog_requests").toULongLong() != before || state->frame().value("focus").toInt() != 2) {
                    finish(false, "Scoped row queries changed focus or bypassed the revision cache"); return;
                }
                finish(true, "Shared command availability, Git pane/row controls, default and remapped global shortcuts, palette/raw commands, menus/window close, dirty quit/discard cancellation and settings routing"); return;
            }
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
            if (!clickMenuAction(window, timer, "Run", "terminal")) {
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

// Desktop-editor behaviour of the graphical interface (commit "Kate parity").
static void startDesktopSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    const QString workspace = dir + "/workspace";
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0);
    auto memory = new QVariantMap;
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json");
        report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}, {"steps", *step}}).toJson());
        window->grabWindow().save(dir + "/gui.png");
        timer->stop();
        QGuiApplication::exit(pass ? 0 : 2);
    };
    auto editorPane = [=]() -> QVariantMap {
        for (const auto &value : state->frame().value("panes").toList())
            if (value.toMap().value("kind") == "editor") return value.toMap();
        return {};
    };
    auto grid = [=]() { return findItem(window->contentItem(), "cells_" + editorPane().value("id").toString()); };
    auto context = [=]() {
        return state->send({{"action", "input_context"}, {"pane", editorPane().value("id")}}).value("editor").toMap();
    };
    auto surfaceText = [=](int pane) {
        QStringList lines;
        for (const auto &line : state->surface(pane).value("lines").toList())
            lines.append(line.toMap().value("layout").toMap().value("text").toString());
        return lines.join("\n");
    };
    auto cellPoint = [=](int row, int column) {
        const int gutter = editorPane().value("rows").toInt() > 0 ? 6 : 0;
        return grid()->mapToScene(QPointF(1 + (gutter + column) * state->cellWidth() + state->cellWidth() / 2.0,
                                          1 + row * state->cellHeight() + state->cellHeight() / 2.0)).toPoint();
    };
    auto write = [](const QString &path, const QByteArray &bytes) {
        QFile file(path);
        file.open(QIODevice::WriteOnly | QIODevice::Truncate);
        file.write(bytes);
    };
    auto read = [](const QString &path) {
        QFile file(path);
        file.open(QIODevice::ReadOnly);
        return file.readAll();
    };
    auto wheel = [=](QPoint at, QPoint pixels, QPoint angle) {
        QWheelEvent event(QPointF(at), window->mapToGlobal(at), pixels, angle, Qt::NoButton, Qt::NoModifier,
                          Qt::ScrollUpdate, false);
        QCoreApplication::sendEvent(window, &event);
    };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        state->refresh();
        if (++*ticks > 400) {
            finish(false, QString("Desktop smoke timeout at step %1: %2").arg(*step).arg(state->frame().value("status").toString()));
            return;
        }
        const auto frame = state->frame();
        const auto pane = editorPane();
        switch (*step) {
        case 0:
            state->send({{"action", "open"}, {"path", workspace + "/edit.txt"}});
            break;
        case 1:
            if (!frame.value("title").toString().contains("edit.txt") || !window->title().contains("Slate")) return;
            grid()->forceActiveFocus();
            memory->insert("catalogs", state->diagnostics().value("catalog_requests"));
            for (const auto c : QString("Hello world "))
                QTest::keyClick(window, c.toLatin1());
            break;
        case 2:
            if (!window->title().contains("edit.txt *")) return;
            // Typing never queries the command catalog: menus read it on open.
            if (state->diagnostics().value("catalog_requests") != memory->value("catalogs")) {
                finish(false, QString("Typing queried the command catalog %1 times")
                    .arg(state->diagnostics().value("catalog_requests").toLongLong() -
                         memory->value("catalogs").toLongLong()));
                return;
            }
            // Double click selects a word, a quick third click the line.
            QTest::mouseDClick(window, Qt::LeftButton, Qt::NoModifier, cellPoint(0, 1));
            break;
        case 3:
            if (context().value("selection") != "Hello") {
                finish(false, "Double click did not select the word: " + context().value("selection").toString());
                return;
            }
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier, cellPoint(0, 1));
            break;
        case 4:
            if (context().value("selection") != "Hello world original\n") {
                finish(false, "Triple click did not select the line: " + context().value("selection").toString());
                return;
            }
            QTest::keyClick(window, Qt::Key_Escape);
            // An external change while the document is modified asks first.
            write(workspace + "/edit.txt", "external change\n");
            break;
        case 5: {
            auto dialog = window->findChild<QObject *>("choiceDialog");
            if (!dialog || !dialog->property("visible").toBool()) return;
            // Click once the dialog has been laid out and painted.
            if (!memory->value("dialog").toBool()) {
                memory->insert("dialog", true);
                return;
            }
            if (frame.value("prompt").toMap().value("kind") != "file-changed") {
                finish(false, "Unexpected prompt for an external change");
                return;
            }
            auto keep = findItem(window->contentItem(), "choiceAlternative");
            if (!keep) keep = [&]() -> QQuickItem * {
                for (auto candidate : window->contentItem()->findChildren<QQuickItem *>())
                    if (candidate->objectName() == "choiceAlternative" && candidate->isVisible()) return candidate;
                return nullptr;
            }();
            if (!keep) { finish(false, "Missing Keep My Version button"); return; }
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier,
                keep->mapToScene(QPointF(keep->width() / 2, keep->height() / 2)).toPoint());
            break;
        }
        case 6:
            if (!frame.value("status").toString().startsWith("Kept your version")) return;
            grid()->forceActiveFocus();
            QTest::keyClick(window, Qt::Key_S, Qt::ControlModifier);
            break;
        case 7:
            if (read(workspace + "/edit.txt") != "Hello world original\n") return;
            // A clean document follows its file automatically.
            write(workspace + "/edit.txt", "changed outside\n");
            break;
        case 8:
            if (!context().value("surrounding").toString().startsWith("changed outside") ||
                !frame.value("status").toString().contains("reloaded")) return;
            memory->insert("cell", state->cellWidth());
            state->send({{"action", "invoke_action"}, {"id", "zoom-in"}});
            break;
        case 9:
            if (frame.value("settings").toMap().value("font_size").toInt() != 12) return;
            if (state->cellWidth() < memory->value("cell").toInt()) { finish(false, "Zoom did not change the font"); return; }
            state->send({{"action", "invoke_action"}, {"id", "zoom-reset"}});
            {
                QByteArray text;
                for (int i = 0; i < 400; ++i) text += "line " + QByteArray::number(i) + "\n";
                text += QByteArray(500, 'w') + "\n";
                write(workspace + "/long.txt", text);
            }
            state->send({{"action", "open"}, {"path", workspace + "/long.txt"}});
            break;
        case 10: {
            if (!frame.value("title").toString().contains("long.txt") || !grid()) return;
            // Touchpad scrolling: many small pixel deltas still scroll.
            const auto at = grid()->mapToScene(QPointF(40, 40)).toPoint();
            for (int i = 0; i < 12; ++i) wheel(at, QPoint(0, -6), QPoint(0, -15));
            break;
        }
        case 11: {
            const int top = pane.value("editor").toMap().value("top").toInt();
            const qreal pixels = grid()->property("scrollPixels").toReal();
            if (top < 2) return;
            if (pixels < 0 || pixels >= state->cellHeight()) { finish(false, "Smooth scroll offset out of range"); return; }
            state->send({{"action", "go_to_line"}, {"line", 401}});
            break;
        }
        case 12: {
            const auto at = grid()->mapToScene(QPointF(40, 40)).toPoint();
            wheel(at, QPoint(-60, 0), QPoint(-120, 0));
            break;
        }
        case 13: {
            if (pane.value("editor").toMap().value("left").toInt() <= 0) return;
            auto minimap = findItem(window->contentItem(), "minimap_" + pane.value("id").toString());
            if (!minimap || !minimap->isVisible()) { finish(false, "Minimap missing"); return; }
            if (minimap->property("widest").toInt() < 500) { finish(false, "Minimap overview is incomplete"); return; }
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier,
                minimap->mapToScene(QPointF(minimap->width() / 2, 4)).toPoint());
            break;
        }
        case 14:
            if (pane.value("editor").toMap().value("top").toInt() != 0) return;
            // Moving the cursor scrolls back to it horizontally.
            grid()->forceActiveFocus();
            QTest::keyClick(window, Qt::Key_Home, Qt::ControlModifier);
            state->send({{"action", "invoke_action"}, {"id", "toggle-whitespace"}});
            break;
        case 15:
            if (!surfaceText(pane.value("id").toInt()).contains(QStringLiteral("line·0"))) return;
            state->send({{"action", "invoke_action"}, {"id", "toggle-whitespace"}});
            if (!frame.value("recent").toList().contains(workspace + "/edit.txt")) {
                finish(false, "Recent files list is missing edit.txt");
                return;
            }
            qputenv("SLATE_GUI_PRINT_PDF", (dir + "/print.pdf").toUtf8());
            state->send({{"action", "invoke_action"}, {"id", "print"}});
            break;
        case 16:
            if (read(dir + "/print.pdf").size() < 1000 || !read(dir + "/print.pdf").startsWith("%PDF")) return;
            QTest::mouseClick(window, Qt::RightButton, Qt::NoModifier, cellPoint(2, 2));
            break;
        case 17: {
            auto menu = window->findChild<QObject *>("editorMenu");
            if (!menu || !menu->property("opened").toBool()) return;
            auto undo = findMenuItem(window, "menuAction_undo");
            auto cut = findMenuItem(window, "menuAction_cut");
            if (!undo || !undo->isVisible() || !cut || !cut->isVisible()) { finish(false, "Editor context menu lacks edit actions"); return; }
            dismissMenu(window);
            // The editor exposes a text interface to assistive technology.
            auto accessible = QAccessible::queryAccessibleInterface(grid());
            if (!accessible || accessible->role() != QAccessible::EditableText || !accessible->textInterface() ||
                accessible->textInterface()->characterCount() == 0 ||
                !accessible->text(QAccessible::Name).contains("long.txt")) {
                finish(false, "Editor accessibility interface is missing");
                return;
            }
            // Dropping files from a file manager opens them.
            auto mime = new QMimeData;
            mime->setUrls({QUrl::fromLocalFile(workspace + "/second.txt")});
            const auto at = grid()->mapToScene(QPointF(30, 30));
            QDragEnterEvent enter(at.toPoint(), Qt::CopyAction, mime, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &enter);
            QDragMoveEvent move(at.toPoint(), Qt::CopyAction, mime, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &move);
            QDropEvent drop(at, Qt::CopyAction, mime, Qt::LeftButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &drop);
            break;
        }
        case 18: {
            if (!frame.value("title").toString().contains("second.txt")) return;
            // Ctrl+A on a non-Latin layout: the key is Cyrillic, the scan code 'a'.
            grid()->forceActiveFocus();
            QKeyEvent press(QEvent::KeyPress, 0x0424, Qt::ControlModifier, 38, 0, 0, QString(QChar(0x0444)));
            QCoreApplication::sendEvent(window, &press);
            break;
        }
        case 19: {
            const auto all = context();
            if (all.value("selection") != all.value("surrounding")) return;
            QTest::keyClick(window, Qt::Key_End, Qt::ControlModifier);
            // Input method composition keeps the IME's styling, then commits.
            QTextCharFormat underline;
            underline.setFontUnderline(true);
            QInputMethodEvent compose(QStringLiteral("ねこ"), {
                QInputMethodEvent::Attribute(QInputMethodEvent::TextFormat, 0, 2, underline),
                QInputMethodEvent::Attribute(QInputMethodEvent::Cursor, 1, 1, QVariant())});
            QCoreApplication::sendEvent(grid(), &compose);
            QInputMethodEvent commit;
            commit.setCommitString(QStringLiteral("猫"));
            QCoreApplication::sendEvent(grid(), &commit);
            break;
        }
        case 20: {
            if (!context().value("surrounding").toString().endsWith(QStringLiteral("猫"))) return;
            // Terminal content has its own theme, independent of the editor.
            state->send({{"action", "configure"}, {"name", "editor-theme"}, {"value", "light"}});
            state->send({{"action", "configure"}, {"name", "terminal-theme"}, {"value", "dark"}});
            state->refresh();
            const auto themed = state->frame();
            if (themed.value("background") != "#fafafa" || themed.value("terminal_background") != "#14171c") {
                finish(false, "Editor and terminal theme settings were not independent");
                return;
            }
            QVariantMap terminal;
            for (const auto &value : frame.value("panes").toList())
                if (value.toMap().value("kind") == "terminal") terminal = value.toMap();
            const auto cells = state->surface(terminal.value("id").toInt()).value("screen").toMap().value("cells").toList();
            if (cells.isEmpty() || cells.last().toList().isEmpty()) return;
            const auto bg = cells.last().toList().last().toMap().value("bg").toString();
            if (bg != themed.value("terminal_background").toString()) {
                finish(false, "Terminal background " + bg + " ignores its theme " + themed.value("terminal_background").toString());
                return;
            }
            qputenv("SLATE_GUI_NEW_WINDOW_LOG", (dir + "/new-window.log").toUtf8());
            state->send({{"action", "invoke_action"}, {"id", "new-window"}});
            break;
        }
        case 21:
            if (!read(dir + "/new-window.log").contains("new-window")) return;
            // Errors stand out in the status bar.
            state->send({{"action", "go_to_line"}, {"line", 999999}});
            break;
        case 22: {
            auto label = findItem(window->contentItem(), "statusLabel");
            if (!label || !label->property("text").toString().startsWith("Error") || !label->property("font").value<QFont>().bold()) return;
            auto minimap = findItem(window->contentItem(), "minimap_2");
            if (!minimap || minimap->property("background").value<QColor>() != QColor("#fafafa")) {
                finish(false, "Minimap did not follow the editor theme");
                return;
            }
            const auto image = window->grabWindow();
            const auto sample = minimap->mapToScene(QPointF(minimap->width() / 2, minimap->height() - 5));
            const auto actualColor = image.pixelColor((sample * image.devicePixelRatio()).toPoint());
            const auto expectedColor = QColor("#fafafa").darker(110);
            if (actualColor.rgba() != expectedColor.rgba()) {
                finish(false, "Minimap retained stale colors after a theme change: " +
                              actualColor.name() + "; expected " + expectedColor.name());
                return;
            }
            if (state->send({{"action", "flush"}}).value("status").toString().isEmpty()) {
                finish(false, "Session flush did not report");
                return;
            }
            finish(true, "Title/modified marker, word and line selection, external change keep/reload, zoom, smooth and horizontal scrolling, minimap, whitespace, recent files, print, context menu, accessibility, drag and drop, non-Latin shortcuts, IME, terminal theme, new window and error status");
            return;
        }
        }
        ++*step;
        *ticks = 0;
    });
    timer->start(100);
}

// IDE tour: quick open, a language server's hover, completion and problems,
// and several carets, with screenshots of each overlay.
static void startIdeSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    const QString workspace = dir + "/workspace";
    auto timer = new QTimer(state);
    auto step = new int(0), ticks = new int(0);
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json");
        report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}, {"steps", *step}}).toJson());
        window->grabWindow().save(dir + "/gui.png");
        timer->stop();
        QGuiApplication::exit(pass ? 0 : 2);
    };
    auto editorPane = [=]() -> QVariantMap {
        QVariantMap any;
        for (const auto &value : state->frame().value("panes").toList()) {
            const auto pane = value.toMap();
            if (pane.value("kind") != "editor") continue;
            if (pane.value("focused").toBool()) return pane;
            if (any.isEmpty()) any = pane;
        }
        return any;
    };
    auto grid = [=]() { return findItem(window->contentItem(), "cells_" + editorPane().value("id").toString()); };
    auto text = [=]() {
        QStringList lines;
        for (const auto &line : state->surface(editorPane().value("id").toInt()).value("lines").toList())
            lines.append(line.toMap().value("layout").toMap().value("text").toString());
        return lines.join("\n");
    };
    auto key = [=](int code, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
        QTest::keyClick(window, Qt::Key(code), modifiers);
    };
    auto type = [=](const QString &value) {
        for (auto c : value) QTest::keyClick(window, c.toLatin1());
    };
    auto shown = [=](const QString &name) {
        auto item = findVisibleItem(window->contentItem(), name);
        return item && item->isVisible() && item->width() > 0 && item->height() > 0 ? item : nullptr;
    };
    // An overlay is entirely inside the window.
    auto inside = [=](QQuickItem *item) {
        const auto r = item->mapRectToScene(QRectF(0, 0, item->width(), item->height()));
        return r.left() >= 0 && r.top() >= 0 && r.right() <= window->width() && r.bottom() <= window->height();
    };
    auto picker = [=]() { return window->findChild<QObject *>("pickerDialog"); };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        state->refresh();
        if (++*ticks > 300) {
            finish(false, QString("IDE smoke timeout at step %1: %2").arg(*step).arg(state->frame().value("status").toString()));
            return;
        }
        const auto frame = state->frame();
        const QString pane = editorPane().value("id").toString();
        switch (*step) {
        case 0:
            state->send({{"action", "open"}, {"path", workspace + "/main.fk"}});
            break;
        case 1:
            if (!frame.value("title").toString().contains("main.fk") ||
                frame.value("problems").toList().value(0).toInt() != 1) return;
            grid()->forceActiveFocus();
            key(Qt::Key_P, Qt::ControlModifier);
            break;
        case 2: {
            if (!picker() || !picker()->property("visible").toBool()) return;
            type("notes");
            break;
        }
        case 3: {
            const auto items = frame.value("picker").toMap().value("items").toList();
            if (items.isEmpty() || items[0].toMap().value("label") != "notes.md") return;
            window->grabWindow().save(dir + "/ide-picker.png");
            key(Qt::Key_Return);
            break;
        }
        case 4:
            if (!frame.value("title").toString().contains("notes.md") || !frame.value("picker").isNull()) return;
            if (picker()->property("visible").toBool()) {
                finish(false, "The picker stayed open after choosing a file");
                return;
            }
            state->send({{"action", "open"}, {"path", workspace + "/main.fk"}});
            break;
        case 5:
            if (!frame.value("title").toString().contains("main.fk")) return;
            grid()->forceActiveFocus();
            key(Qt::Key_Home, Qt::ControlModifier);
            key(Qt::Key_Right);
            key(Qt::Key_Right);
            key(Qt::Key_Right);
            key(Qt::Key_Right);
            key(Qt::Key_Right);
            key(Qt::Key_H, Qt::AltModifier);
            break;
        case 6: {
            auto hover = shown("hover_" + pane);
            if (!hover) return;
            if (!inside(hover) || !shown("hoverText_" + pane)->property("text").toString().contains("hover for greet")) {
                finish(false, "Hover text missing or outside the window");
                return;
            }
            window->grabWindow().save(dir + "/ide-hover.png");
            key(Qt::Key_Escape);
            break;
        }
        case 7:
            if (shown("hover_" + pane)) return;
            // The end of the second line (the file ends with a newline).
            key(Qt::Key_End, Qt::ControlModifier);
            key(Qt::Key_Backspace);
            type(" gr");
            break;
        case 8: {
            auto box = shown("completion_" + pane);
            if (!box) return;
            if (!inside(box)) {
                finish(false, "Completion list outside the window");
                return;
            }
            window->grabWindow().save(dir + "/ide-completion.png");
            key(Qt::Key_Tab);
            break;
        }
        case 9: {
            if (shown("completion_" + pane) || !text().contains("ERROR greet")) return;
            auto problems = shown("problemCount");
            if (!problems || !problems->property("text").toString().contains("1")) {
                finish(false, "Problem count missing from the status bar");
                return;
            }
            const auto center = problems->mapToScene(QPointF(problems->width() / 2, problems->height() / 2)).toPoint();
            QTest::mouseClick(window, Qt::LeftButton, Qt::NoModifier, center);
            break;
        }
        case 10: {
            const auto items = frame.value("picker").toMap().value("items").toList();
            if (items.isEmpty() || items[0].toMap().value("label") != "an error here") return;
            window->grabWindow().save(dir + "/ide-problems.png");
            key(Qt::Key_Escape);
            break;
        }
        case 11:
            if (!frame.value("picker").isNull()) return;
            grid()->forceActiveFocus();
            key(Qt::Key_Home, Qt::ControlModifier);
            key(Qt::Key_Down, Qt::ControlModifier | Qt::AltModifier);
            type("X");
            break;
        case 12:
            if (!text().contains("Xdef greet") || !text().contains("Xgreet ERROR")) return;
            window->grabWindow().save(dir + "/ide-carets.png");
            key(Qt::Key_Escape);
            // Resting the mouse on a word shows its hover text.
            QTest::mouseMove(window, grid()->mapToScene(QPointF(
                1 + 8 * state->cellWidth(), 1 + state->cellHeight() / 2.0)).toPoint());
            break;
        case 13: {
            auto hover = shown("hover_" + pane);
            if (!hover || !shown("hoverText_" + pane)->property("text").toString().contains("hover for")) return;
            // Leaving the text hides it again.
            QTest::mouseMove(window, QPoint(window->width() / 2, window->height() - 4));
            break;
        }
        case 14:
            if (shown("hover_" + pane)) return;
            if (!frame.value("title").toString().contains("main.fk")) {
                finish(false, "Expected main.fk before switching tabs");
                return;
            }
            // Ctrl+Tab reaches the editor instead of moving keyboard focus.
            grid()->forceActiveFocus();
            key(Qt::Key_Tab, Qt::ControlModifier);
            break;
        case 15:
            if (frame.value("title").toString().contains("main.fk")) return;
            finish(true, "Quick open picker, hover, completion, problems list and status count, several carets, mouse hover, Ctrl+Tab");
            return;
        }
        ++*step;
        *ticks = 0;
    });
    timer->start(100);
}

static void startAuditSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR"), workspace = dir + "/workspace";
    auto timer = new QTimer(state); auto step = std::make_shared<int>(0); auto ticks = std::make_shared<int>(0);
    auto finish = [=](bool pass, const QString &detail) {
        timer->stop(); QFile report(dir + "/report.json"); report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass", pass}, {"detail", detail}}).toJson());
        window->grabWindow().save(dir + "/gui.png"); QGuiApplication::exit(pass ? 0 : 2);
    };
    auto invoke = [=](const QString &id, const QString &argument = QString()) { state->send({{"action", "invoke_action"}, {"id", id}, {"argument", argument}}); state->refresh(); };
    auto select = [=](const QString &path) {
        state->refresh();
        int pane = -1;
        for (const auto &value : state->frame().value("panes").toList()) {
            auto item = value.toMap();
            if (item.value("kind") == "files") { pane = item.value("id").toInt(); break; }
        }
        if (pane < 0) return false;
        state->send({{"action", "focus"}, {"pane", pane}});
        state->refresh();
        const auto entries = state->frame().value("files").toList();
        for (int i = 0; i < entries.size(); ++i) if (entries[i].toMap().value("path").toString() == path) {
            state->send({{"action", "click"}, {"pane", pane}, {"row", i}, {"col", 0}}); state->refresh(); return true;
        }
        return false;
    };
    auto pathDialog = []() -> QFileDialog * { for (auto w : QApplication::topLevelWidgets()) if (auto d = qobject_cast<QFileDialog *>(w); d && d->objectName() == "pathDialog") return d; return nullptr; };
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        if (++*ticks > 300) { finish(false, QString("Audit timeout at %1: %2").arg(*step).arg(state->frame().value("status").toString())); return; }
        switch (*step) {
        case 0: if (*ticks < 5) return; state->send({{"action", "focus"}, {"pane", 1}}); invoke("new-folder", "nested"); break;
        case 1: if (!QFileInfo::exists(workspace + "/nested")) return; if (!select(workspace + "/nested")) return; invoke("toggle-folder"); invoke("new-file", "nested/inside.txt"); break;
        case 2: {
            if (!QFileInfo::exists(workspace + "/nested/inside.txt")) return;
            bool child = false; for (const auto &row : state->frame().value("files").toList()) if (row.toMap().value("depth").toInt() == 1) child = true;
            if (!child || !select(workspace + "/nested/inside.txt")) return;
            auto menu = findItem(window->contentItem(), "paneActions_1");
            if (!menu) { finish(false, "Explorer context menu missing"); return; }
            invoke("rename-file", "renamed.txt"); break;
        }
        case 3:
            if (!QFileInfo::exists(workspace + "/nested/renamed.txt")) return;
            if (QFileInfo::exists(workspace + "/nested/inside.txt")) { finish(false, "Rename left the old file behind"); return; }
            state->send({{"action", "focus"}, {"pane", 2}});
            state->send({{"action", "paste"}, {"text", QString(6000, 'q') + QString::fromUtf8("猫👩‍💻")}}); state->refresh(); break;
        case 4: {
            auto grid = qobject_cast<CellView *>(findItem(window->contentItem(), "cells_2"));
            auto accessible = grid ? QAccessible::queryAccessibleInterface(grid) : nullptr; auto text = accessible ? accessible->textInterface() : nullptr;
            if (!text || text->characterCount() != 6006 || text->text(0, 12) != QString(12, 'q')) { finish(false, "Accessibility lacks full document ranges/UTF-16 offsets"); return; }
            text->setCursorPosition(0); if (text->cursorPosition() != 0) { finish(false, "Accessible cursor movement failed"); return; }
            text->setSelection(0, 6000, 6006); int start = -1, end = -1; text->selection(0, &start, &end);
            if (start != 6000 || end != 6006 || text->text(start, end) != QString::fromUtf8("猫👩‍💻")) { finish(false, "Accessible Unicode selection failed"); return; }
            text->setCursorPosition(0); auto rect = text->characterRect(0);
            if (!rect.isValid() || text->offsetAtPoint(rect.center()) != 0) { finish(false, "Accessible character geometry/hit testing failed"); return; }
            text->scrollToSubstring(6000, 6006); if (text->cursorPosition() != 0) { finish(false, "Accessible scrolling moved the cursor"); return; }
            invoke("new"); state->send({{"action", "paste"}, {"text", "first untitled"}}); invoke("new"); state->send({{"action", "paste"}, {"text", "second untitled"}}); invoke("save-all"); break;
        }
        case 5: { auto dialog = pathDialog(); if (!dialog) return; selectDialogPath(workspace + "/first-saved.txt"); break; }
        case 6: if (!QFileInfo::exists(workspace + "/first-saved.txt")) return; break;
        case 7: { auto dialog = pathDialog(); if (!dialog) return; selectDialogPath(workspace + "/second-saved.txt"); break; }
        case 8:
            if (!QFileInfo::exists(workspace + "/second-saved.txt") || state->frame().value("dirty").toBool()) return;
            state->send({{"action", "paste"}, {"text", "changed "}}); invoke("close"); break;
        case 9: {
            QMessageBox *dialog = nullptr; for (auto w : QApplication::topLevelWidgets()) if (w->objectName() == "closeTabDialog") dialog = qobject_cast<QMessageBox *>(w);
            if (!dialog) return;
            if (!dialog->button(QMessageBox::Save)) { finish(false, "Modified tab has no Save button"); return; }
            QTest::mouseClick(dialog->button(QMessageBox::Save), Qt::LeftButton); break;
        }
        case 10:
            if (state->closeDialogOpen() || state->frame().value("dirty").toBool()) return;
            invoke("profile-save", "audit-profile"); invoke("profile-load", "minimal"); break;
        case 11:
            if (!state->frame().value("editor_only").toBool()) { finish(false, "Minimal profile did not collapse panes"); return; }
            invoke("profile-load", "audit-profile"); break;
        case 12:
            if (state->frame().value("editor_only").toBool()) { finish(false, "Saved profile did not restore panes"); return; }
            if (!select(workspace + "/first-saved.txt")) return;
            invoke("trash-file");
            if (state->frame().value("prompt").toMap().value("kind") != "trash-file") {
                finish(false, QString("Trash unavailable: %1").arg(state->frame().value("status").toString())); return;
            }
            break;
        case 13: {
            auto prompt = state->frame().value("prompt").toMap(); if (prompt.value("kind") != "trash-file") return;
            state->send({{"action", "submit_prompt"}, {"all", false}}); state->refresh(); break;
        }
        case 14:
            if (QFileInfo::exists(workspace + "/first-saved.txt")) return;
            finish(true, "Explorer expansion/create/rename/desktop Trash, full Unicode accessibility, native Save All dialogs, Save-and-close and profile restoration"); return;
        }
        ++*step;
    });
    timer->start(100);
}
static void startProductionSmoke(Bridge *state, QQuickWindow *window) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state); auto step = new int(0), ticks = new int(0);
    auto finish = [=](bool pass, const QString &detail) {
        QFile report(dir + "/report.json"); report.open(QIODevice::WriteOnly);
        report.write(QJsonDocument(QJsonObject{{"pass",pass},{"detail",detail},{"steps",*step}}).toJson());
        timer->stop(); QGuiApplication::exit(pass ? 0 : 2);
    };
    auto invoke = [=](const QString &id) { QMetaObject::invokeMethod(window,"invokeAction",Q_ARG(QVariant,QVariant(id)),Q_ARG(QVariant,QVariant("")),Q_ARG(QVariant,QVariant(0)),Q_ARG(QVariant,QVariant(-1))); };
    auto control = [=](const QString &name) {return findItem(window->contentItem(),name);};
    auto click = [=](const QString &name) {auto item=control(name); if (!item) return false; QTest::mouseClick(window,Qt::LeftButton,Qt::NoModifier,item->mapToScene(QPointF(item->width()/2,item->height()/2)).toPoint()); return true;};
    QObject::connect(timer,&QTimer::timeout,state,[=]() {
        if (++*ticks > 100) { finish(false,QString("Production smoke timed out at %1: %2").arg(*step).arg(state->frame().value("status").toString())); return; }
        state->refresh();
        if (qEnvironmentVariableIsSet("SLATE_GUI_GEOMETRY_SMOKE")) {
            if (*ticks < 8) return;
            if (qEnvironmentVariable("SLATE_GUI_GEOMETRY_PHASE") == "save") {window->setGeometry(80,70,600,400);finish(true,"Saved normal window geometry");}
            else {finish(window->geometry() == QRect(80,70,600,400),"Restored window geometry");}
            return;
        }
        switch (*step) {
        case 0: state->send({{"action","open"},{"path",dir+"/workspace/edit.txt"}}); break;
        case 1:
            if (state->send({{"action","document_text"}}).value("text") != "original\n") return;
            invoke("select-all");
            state->send({{"action","paste"},{"text","x=12\ny=34\n"}});
            state->send({{"action","search"},{"query","x=12"},{"case_sensitive",true},{"whole_word",false},{"backward",false}});
            invoke("prompt-replace"); break;
        case 2: {
            if (!control("regexSearch") || !control("selectionSearch")) return;
            click("selectionSearch");click("regexSearch");
            auto input=control("searchInput"); input->setProperty("text","([xy])=(\\d+)");QMetaObject::invokeMethod(input,"textEdited");
            auto replacement=control("replacementInput");replacement->setProperty("text","$1:$2");QMetaObject::invokeMethod(replacement,"textEdited");break;
        }
        case 3: {
            const auto options=state->frame().value("search").toMap();
            if (!options.value("regex_mode").toBool() || !options.value("selection_only").toBool()) {finish(false,"Regex and selection controls did not configure shared search");return;}
            if (!click("replaceAll")) return;break;
        }
        case 4:
            if (state->send({{"action","document_text"}}).value("text") != "x:12\ny=34\n") {finish(false,"Selection-scoped capture replacement changed the wrong text: " + QJsonDocument::fromVariant(state->send({{"action","document_text"}})).toJson() + QJsonDocument::fromVariant(state->frame().value("prompt")).toJson() + state->frame().value("status").toString());return;}
            state->send({{"action","dismiss_prompt"}});invoke("settings");break;
        case 5:
            if (!control("shortcutChord")) return;
            control("shortcutChord")->setProperty("text","Ctrl+Alt+j");control("shortcutCommand")->setProperty("text","word-count");
            QMetaObject::invokeMethod(control("saveShortcut"),"clicked");break;
        case 6: {
            if (state->frame().value("settings").toMap().value("global_keys").toMap().value("Ctrl+Alt+j") != "word-count") {finish(false,"Graphical shortcut editor failed to persist binding");return;}
            auto dialog=window->findChild<QObject *>("settingsDialog");QMetaObject::invokeMethod(dialog,"close");invoke("search-in-files");break;
        }
        case 7:
            if (!control("projectInclude")) return;
            control("projectInclude")->setProperty("text","*.txt");QMetaObject::invokeMethod(control("projectInclude"),"editingFinished");
            click("projectHidden");break;
        case 8: {
            const auto policy=state->frame().value("project_search_policy").toMap();
            if (policy.value("include") != "*.txt" || !policy.value("hidden").toBool()) {finish(false,"Project search controls failed to configure scope");return;}
            finish(true,"Regex/selection controls, capture replacement, graphical shortcut editor and project search policy");return;
        }
        }
        ++*step;
    }); timer->start(100);
}
// A test-only driver lets the process smoke close tabs through the real GUI bridge.
static void startWaitSmoke(Bridge *state) {
    const QString dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    auto timer = new QTimer(state);
    auto sequence = new int(0);
    auto response = new QVariantMap;
    QObject::connect(timer, &QTimer::timeout, state, [=]() {
        QFile input(dir + "/wait-command.json");
        if (input.open(QIODevice::ReadOnly)) {
            const auto request = QJsonDocument::fromJson(input.readAll()).object();
            const int incoming = request.value("sequence").toInt();
            if (incoming > *sequence) {
                *response = state->send(request.value("command").toObject().toVariantMap());
                *sequence = incoming;
            }
        }
        state->refresh();
        QVariantMap report{{"pid", QCoreApplication::applicationPid()}, {"sequence", *sequence},
                           {"response", *response}, {"frame", state->frame()},
                           {"document", state->send({{"action", "document_text"}})}};
        QFile output(dir + "/wait-state.json.tmp");
        if (output.open(QIODevice::WriteOnly)) {
            output.write(QJsonDocument::fromVariant(report).toJson()); output.close();
            // POSIX rename atomically replaces the last observation.
            ::rename((dir + "/wait-state.json.tmp").toLocal8Bit().constData(),
                     (dir + "/wait-state.json").toLocal8Bit().constData());
        }
    });
    timer->start(50);
}
void startSmoke(Bridge *state, QQuickWindow *window) {
    if (qEnvironmentVariableIsSet("SLATE_GUI_WAIT_SMOKE")) { startWaitSmoke(state); return; }
    if (qEnvironmentVariableIsSet("SLATE_GUI_PRODUCTION_SMOKE") || qEnvironmentVariableIsSet("SLATE_GUI_GEOMETRY_SMOKE")) {startProductionSmoke(state,window);return;}
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_AUDIT_SMOKE")) { startAuditSmoke(state, window); return; }
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
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_DESKTOP_SMOKE")) {
        startDesktopSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_IDE_SMOKE")) {
        startIdeSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_FILE_DIALOG_SMOKE")) {
        startFileDialogSmoke(state, window);
        return;
    }
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_COMMAND_SMOKE")) {
        startCommandSmoke(state, window);
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
            int fileIndex = -1;
            const auto files = state->frame().value("files").toList();
            for (int i = 0; i < files.size(); ++i)
                if (files[i].toMap().value("name") == "edit.txt") fileIndex = i;
            if (fileIndex < 0) return;
            if (state->paneIds().size() != 3) {
                finish(false, "Default workspace did not contain three panes");
                return;
            }
            auto browser = findItem(window->contentItem(), "browser_1");
            if (!browser) {
                finish(false, "Missing file browser");
                return;
            }
            click(browser, QPointF(60, window->property("fileRowHeight").toInt() * (fileIndex + 0.5)), true);
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
                finish(false, "Find dialog did not select expected text: " + frame.value("status").toString());
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
                finish(false, "Replace dialog did not edit the selected match: " + QJsonDocument::fromVariant(state->send({{"action","document_text"}})).toJson() + QJsonDocument::fromVariant(state->frame().value("prompt")).toJson() + state->frame().value("status").toString());
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
