// Only compiled with gui-smoke. Measure real input handlers and Qt frame swaps.
#include "bridge.h"
#include <QCoreApplication>
#include <QElapsedTimer>
#include <QFile>
#include <QGuiApplication>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QQuickWindow>
#include <QTimer>
#include <algorithm>
#include <memory>

static CellView *editorItem(QQuickItem *item, int id) {
    if (item->objectName() == QString("cells_%1").arg(id)) return qobject_cast<CellView *>(item);
    for (auto child : item->childItems()) if (auto found = editorItem(child, id)) return found;
    return nullptr;
}
static double percentile(QList<double> values, double quantile) {
    if (values.isEmpty()) return 0;
    std::sort(values.begin(), values.end());
    return values[qBound(0, int((values.size()-1)*quantile), int(values.size()-1))];
}
struct PerformanceRun {
    QElapsedTimer clock;
    QList<double> frames, dispatches;
    bool pending = false;
    qint64 started = 0;
    int keys = 0;
    qulonglong dispatchedUpdate = 0;
    qulonglong beforeLayouts = 0, beforeUpdates = 0, beforeClipboard = 0, maximumBytes = 0;
};
void startPerformanceSmoke(Bridge *state, QQuickWindow *window) {
    const auto dir = qEnvironmentVariable("SLATE_GUI_SMOKE_DIR");
    const auto mode = qEnvironmentVariable("SLATE_GUI_PERF_MODE", "typing");
    const auto size = qEnvironmentVariable("SLATE_GUI_PERF_SIZE", "1360x820").split('x');
    window->resize(size[0].toInt(), size[1].toInt());
    auto run = std::make_shared<PerformanceRun>();
    run->clock.start();
    QObject::connect(window, &QQuickWindow::frameSwapped, state, [=]() {
        if (run->pending && state->diagnostics().value("updates").toULongLong() > run->dispatchedUpdate) {
            run->frames.append((run->clock.nsecsElapsed()-run->started)/1e6);
            run->pending = false;
        }
    });
    QObject::connect(state, &Bridge::refreshFinished, state, [=]() {
        run->maximumBytes = qMax(run->maximumBytes, state->diagnostics().value("last_update_bytes").toULongLong());
    });
    QTimer::singleShot(1500, state, [=]() {
        const int id = state->frame().value("focus").toInt();
        auto grid = editorItem(window->contentItem(), id);
        if (!grid) { QGuiApplication::exit(3); return; }
        grid->forceActiveFocus();
        window->grabWindow(); // Warm font/layout and renderer initialization before timing.
        const QString file = dir + "/workspace/" + (mode == "syntax" ? "edit.rs" : "edit.txt");
        QFile original(file); original.open(QIODevice::ReadOnly);
        auto expected = std::make_shared<QByteArray>(original.readAll());
        if (mode == "busy") {
            state->send({{"action", "focus"}, {"pane", 3}});
            state->send({{"action", "paste"}, {"text", "i=0; while [ \"$i\" -lt 80 ]; do printf 'busy %s\\n' \"$i\"; i=$((i+1)); sleep .03; done\r"}});
            state->send({{"action", "focus"}, {"pane", id}});
            state->refresh();
        }
        run->beforeLayouts = grid->layoutBuilds();
        const auto diagnostics = state->diagnostics();
        run->beforeUpdates = diagnostics.value("updates").toULongLong();
        run->beforeClipboard = diagnostics.value("clipboard_reads").toULongLong();
        run->maximumBytes = 0;
        const bool typing = mode == "typing" || mode == "syntax" || mode == "burst" || mode == "busy";
        auto send = [=](int index) {
            if (!run->pending) {
                run->started = run->clock.nsecsElapsed();
                run->dispatchedUpdate = state->diagnostics().value("updates").toULongLong();
            }
            run->pending = true;
            QElapsedTimer elapsed; elapsed.start();
            if (mode == "scroll") {
                state->send({{"action", "scroll"}, {"pane", id}, {"delta", index % 2 ? -3 : 3}});
                state->scheduleRefresh();
            } else {
                const auto key = typing ? Qt::Key_X : index % 2 ? Qt::Key_Left : Qt::Key_Right;
                QKeyEvent event(QEvent::KeyPress, key, mode == "selection" ? Qt::ShiftModifier : Qt::NoModifier, typing ? "x" : QString());
                QCoreApplication::sendEvent(grid, &event);
            }
            run->dispatches.append(elapsed.nsecsElapsed()/1e6);
            ++run->keys;
        };
        auto finish = [=]() {
            const auto diag = state->diagnostics();
            const auto updates = diag.value("updates").toULongLong()-run->beforeUpdates;
            const auto clipboardReads = diag.value("clipboard_reads").toULongLong()-run->beforeClipboard;
            const auto layouts = grid->layoutBuilds()-run->beforeLayouts;
            const double medianFrame = percentile(run->frames, .5), p95Frame = percentile(run->frames, .95);
            const double medianDispatch = percentile(run->dispatches, .5), p95Dispatch = percentile(run->dispatches, .95);
            QString error;
            if (run->frames.isEmpty()) error = "No rendered input frame";
            else if (mode != "burst" && run->frames.size() != run->keys) error = "Input frames were dropped at the paced test rate";
            else if (medianDispatch > 10 || p95Dispatch > 30) error = "Input handler blocks the UI thread";
            else if (medianFrame > 75 || p95Frame > 150) error = "Input-to-frame latency exceeded the release budget";
            else if (mode != "busy" && run->maximumBytes > 32768) error = "Short documents produced oversized updates";
            else if (clipboardReads != 0) error = "Typing/navigation/save accessed the system clipboard";
            else if ((mode == "cursor" || mode == "selection") && layouts != 0) error = "Cursor/selection changes rebuilt text layouts";
            else if (mode == "syntax" && layouts > qulonglong(run->keys * 2 + 4)) error = "Highlighting invalidated unchanged text layouts";
            else if (mode == "burst" && updates > 10) error = "Burst inputs were not coalesced";
            QJsonObject report{{"pass", error.isEmpty()}, {"detail", error.isEmpty() ? "Responsive input, compact native row updates and retained layouts" : error},
                               {"mode", mode}, {"width", window->width()}, {"height", window->height()},
                               {"keys", run->keys}, {"frames", run->frames.size()},
                               {"median_dispatch_ms", medianDispatch}, {"p95_dispatch_ms", p95Dispatch},
                               {"median_frame_ms", medianFrame}, {"p95_frame_ms", p95Frame},
                               {"layout_builds", qint64(layouts)}, {"updates", qint64(updates)},
                               {"max_update_bytes", qint64(run->maximumBytes)}, {"clipboard_reads", qint64(clipboardReads)}};
            QFile output(dir + "/report.json"); output.open(QIODevice::WriteOnly);
            output.write(QJsonDocument(report).toJson());
            QGuiApplication::exit(error.isEmpty() ? 0 : 2);
        };
        auto saveAndFinish = [=]() {
            if (typing) {
                expected->prepend(QByteArray(run->keys, 'x'));
                QKeyEvent save(QEvent::KeyPress, Qt::Key_S, Qt::ControlModifier);
                QCoreApplication::sendEvent(grid, &save);
            }
            auto check = new QTimer(state);
            auto attempts = std::make_shared<int>(0);
            QObject::connect(check, &QTimer::timeout, state, [=]() {
                QFile saved(file); saved.open(QIODevice::ReadOnly);
                if (saved.readAll() != *expected) {
                    if (++*attempts > 100) { check->stop(); QGuiApplication::exit(4); }
                    return;
                }
                check->stop(); finish();
            });
            check->start(25);
        };
        if (mode == "burst") {
            for (int i=0; i<100; ++i) send(i);
            QTimer::singleShot(350, state, saveAndFinish);
        } else {
            auto timer = new QTimer(state);
            timer->setTimerType(Qt::PreciseTimer);
            QObject::connect(timer, &QTimer::timeout, state, [=]() {
                if (run->keys == 30) { timer->stop(); QTimer::singleShot(200, state, saveAndFinish); return; }
                send(run->keys);
            });
            timer->start(100);
        }
    });
}
