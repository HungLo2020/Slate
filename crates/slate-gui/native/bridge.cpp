#include "bridge.h"
#include <QApplication>
#include <QClipboard>
#include <QFontDatabase>
#include <QFontMetrics>
#include <QGuiApplication>
#include <QHoverEvent>
#include <QInputMethod>
#include <QInputMethodEvent>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPainter>
#include <QPalette>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickWindow>
#include <QSocketNotifier>
#include <QTextCharFormat>
#include <QTextLayout>
#include <QTimer>
#include <QWheelEvent>
#include <cmath>
#include <cstdio>
extern "C" int slate_event_fd(void *);
extern "C" char *slate_request(void *, const char *);
extern "C" void slate_response_free(char *);
static Bridge *bridge = nullptr;
#ifdef SLATE_SMOKE_TEST
void startSmoke(Bridge *, QQuickWindow *);
#endif
void EntryModel::replace(const QVariantList &rows) {
    if (rows == m_rows)
        return;
    if (rows.size() != m_rows.size()) {
        beginResetModel();
        m_rows = rows;
        endResetModel();
        return;
    }
    for (int i = 0; i < rows.size(); ++i) {
        if (rows[i] != m_rows[i]) {
            m_rows[i] = rows[i];
            emit dataChanged(index(i), index(i));
        }
    }
}
QVariantList Bridge::commands(const QString &query) {
    return send({{"action", "catalog"}, {"query", query}}).value("commands").toList();
}
Bridge::Bridge(void *context, QObject *parent) : QObject(parent), m_context(context) {}
QFont Bridge::font() const {
    auto f = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    f.setPointSize(11);
    f.setStyleHint(QFont::Monospace);
    return f;
}
int Bridge::cellWidth() const {
    return int(std::ceil(QFontMetricsF(font()).horizontalAdvance("M")));
}
int Bridge::cellHeight() const { return int(std::ceil(QFontMetricsF(font()).height())); }
QVariantMap Bridge::send(const QVariantMap &command) {
    const auto bytes =
        QJsonDocument(QJsonObject::fromVariantMap(command)).toJson(QJsonDocument::Compact);
    char *raw = slate_request(m_context, bytes.constData());
    const auto result = QJsonDocument::fromJson(QByteArray(raw)).object().toVariantMap();
    slate_response_free(raw);
    const auto action = command.value("action").toString();
    if (action == "reload_settings" ||
        (action == "invoke_action" && command.value("id") == "settings-reload") ||
        (action == "configure" && command.value("name") == "theme" &&
         command.value("value") == "auto") ||
        (action == "command" && (command.value("text").toString().trimmed() == "settings-reload" ||
                                 command.value("text").toString().trimmed() == "set theme auto")))
        applyTheme();
    return result;
}
void Bridge::command(const QString &text) {
    send({{"action", "command"}, {"text", text}});
    refresh();
}
void Bridge::viewport(int width, int height) {
    if (m_width == width && m_height == height)
        return;
    m_width = qBound(1, width, 65535);
    m_height = qBound(1, height, 65535);
    refresh();
}
void Bridge::paneHeader(int height) { m_headerHeight = qBound(1, height, 65535); }
void Bridge::applyTheme() {
    const auto palette = QGuiApplication::palette();
    const QVariantMap theme{
        {"action", "theme"},
        {"foreground", palette.color(QPalette::Text).name()},
        {"background", palette.color(QPalette::Base).name()},
        {"selection", palette.color(QPalette::Highlight).name()},
        {"selection_foreground", palette.color(QPalette::HighlightedText).name()},
        {"accent", palette.color(QPalette::Highlight).name()}};
    if (theme == m_theme)
        return;
    m_theme = theme;
    send(theme);
}
void Bridge::refresh() {
    auto frame = send({{"action", "update"},
                       {"width", m_width},
                       {"height", m_height},
                       {"cell_width", cellWidth()},
                       {"cell_height", cellHeight()},
                       {"header_height", m_headerHeight},
                       {"minimum_width", qMax(160, m_headerHeight * 4)}});
    if (frame.value("unchanged").toBool())
        return;
    // Missing components in an update retain their previous model/screen.
    if (!frame.contains("files"))
        frame.insert("files", m_frame.value("files"));
    if (!frame.contains("git"))
        frame.insert("git", m_frame.value("git"));
    QVariantList patched;
    for (const auto &item : frame.value("panes").toList()) {
        auto pane = item.toMap();
        if (!pane.contains("screen")) {
            for (const auto &prior : m_frame.value("panes").toList()) {
                if (prior.toMap().value("id") == pane.value("id") &&
                    prior.toMap().value("kind") == pane.value("kind")) {
                    pane.insert("screen", prior.toMap().value("screen"));
                    break;
                }
            }
        }
        patched.append(pane);
    }
    frame.insert("panes", patched);
    {
        QVariantList panes, handles;
        for (const auto &p : frame.value("panes").toList())
            panes.append(p.toMap().value("id"));
        for (const auto &h : frame.value("handles").toList())
            handles.append(h.toMap().value("id"));
        const bool structure = panes != m_paneIds || handles != m_handleIds;
        const bool fileChange = frame.value("files") != m_frame.value("files"),
                   gitChange = frame.value("git") != m_frame.value("git");
        m_frame = frame;
        m_paneIds = panes;
        m_handleIds = handles;
        if (structure)
            emit structureChanged();
        if (fileChange) {
            m_files.replace(m_frame.value("files").toList());
            emit filesChanged();
        }
        if (gitChange) {
            m_git.replace(m_frame.value("git").toList());
            emit gitChanged();
        }
        emit frameChanged();
    }
    if (frame.value("quit").toBool())
        QCoreApplication::quit();
}
void Bridge::copyClipboard() {
    QGuiApplication::clipboard()->setText(send({{"action", "copy"}}).value("clipboard").toString());
    refresh();
}
void Bridge::pasteClipboard() {
    send({{"action", "paste"}, {"text", QGuiApplication::clipboard()->text()}});
    refresh();
}
void Bridge::exit() {
    send({{"action", "quit"}, {"force", false}});
    refresh();
}
CellView::CellView(QQuickItem *parent) : QQuickPaintedItem(parent) {
    setAcceptedMouseButtons(Qt::LeftButton | Qt::MiddleButton | Qt::RightButton);
    setAcceptHoverEvents(true);
    setFlag(ItemAcceptsInputMethod, true);
    setActiveFocusOnTab(true);
    setClip(true);
}
void CellView::setPane(const QVariantMap &pane) {
    if (m_pane == pane)
        return;
    if (m_pane.value("revision") != pane.value("revision") ||
        m_pane.value("kind") != pane.value("kind"))
        m_layoutDirty = true;
    m_pane = pane;
    if (hasActiveFocus() && QGuiApplication::inputMethod())
        QGuiApplication::inputMethod()->update(Qt::ImQueryAll);
    emit paneChanged();
    update();
}
int CellView::textPosition(int row, int column) const {
    if (row < 0 || row >= int(m_columns.size()))
        return 0;
    const auto &columns = m_columns[row];
    for (int i = 0; i < columns.size(); ++i)
        if (columns[i] >= column)
            return i;
    return qMax(0, int(columns.size()) - 1);
}
qreal CellView::cursorX(int row, int column) const {
    layoutText();
    if (row < 0 || row >= int(m_lines.size()))
        return column * bridge->cellWidth();
    return m_lines[row]->lineAt(0).cursorToX(textPosition(row, column));
}
void CellView::layoutText() const {
    if (!m_layoutDirty || !bridge || m_pane.value("kind") != "editor")
        return;
    m_lines.clear();
    m_columns.clear();
    const auto rows = m_pane.value("screen").toMap().value("cells").toList();
    const auto cursor = m_pane.value("screen").toMap().value("cursor").toList();
    for (int row = 0; row < rows.size(); ++row) {
        QString text;
        QVector<int> columns;
        QList<QTextLayout::FormatRange> formats;
        const auto cells = rows[row].toList();
        for (int col = 0; col < cells.size(); ++col) {
            const auto cell = cells[col].toMap();
            if (cell.value("continuation").toBool())
                continue;
            QString glyph = cell.value("text").toString();
            if (glyph.isEmpty())
                glyph = " ";
            QTextCharFormat format;
            format.setForeground(QColor(cell.value("fg").toString()));
            format.setBackground(QColor(cell.value("bg").toString()));
            format.setFontWeight(cell.value("bold").toBool() ? QFont::Bold : QFont::Normal);
            format.setFontItalic(cell.value("italic").toBool());
            format.setFontUnderline(cell.value("underline").toBool());
            if (!formats.isEmpty() && formats.last().format == format)
                formats.last().length += glyph.size();
            else
                formats.append({int(text.size()), int(glyph.size()), format});
            for (int i = 0; i < glyph.size(); ++i)
                columns.append(col);
            text += glyph;
        }
        columns.append(cells.size());
        auto layout = std::make_unique<QTextLayout>(text, bridge->font());
        QTextOption option;
        option.setWrapMode(QTextOption::NoWrap);
        option.setTextDirection(Qt::LeftToRight);
        layout->setTextOption(option);
        layout->setFormats(formats);
        layout->setCacheEnabled(true);
        m_columns.push_back(columns);
        if (cursor.size() == 2 && cursor[0].toInt() == row && !m_preedit.isEmpty())
            layout->setPreeditArea(textPosition(row, cursor[1].toInt()), m_preedit);
        layout->beginLayout();
        auto line = layout->createLine();
        line.setLineWidth(1000000);
        layout->endLayout();
        m_lines.push_back(std::move(layout));
    }
    m_layoutDirty = false;
}
void CellView::paint(QPainter *p) {
    if (!bridge)
        return;
    const bool editor = m_pane.value("kind") == "editor";
    p->fillRect(boundingRect(), editor ? QColor(bridge->frame().value("background").toString())
                                       : QColor("#20242c"));
    const auto screen = m_pane.value("screen").toMap();
    const int cw = bridge->cellWidth(), ch = bridge->cellHeight();
    if (editor) {
        layoutText();
        for (int row = 0; row < int(m_lines.size()); ++row)
            m_lines[row]->draw(p, QPointF(1, 1 + row * ch));
    } else {
        const auto rows = screen.value("cells").toList();
        const auto base = bridge->font();
        const auto ascent = QFontMetrics(base).ascent();
        for (int row = 0; row < rows.size(); ++row) {
            const auto line = rows[row].toList();
            for (int col = 0; col < line.size(); ++col) {
                const auto cell = line[col].toMap();
                QRect rect(1 + col * cw, 1 + row * ch, cw, ch);
                p->fillRect(rect, QColor(cell.value("bg").toString()));
                if (cell.value("continuation").toBool())
                    continue;
                auto f = base;
                f.setBold(cell.value("bold").toBool());
                f.setItalic(cell.value("italic").toBool());
                f.setUnderline(cell.value("underline").toBool());
                p->setFont(f);
                p->setPen(QColor(cell.value("fg").toString()));
                p->drawText(rect.x(), rect.y() + ascent, cell.value("text").toString());
            }
        }
    }
    const auto cursor = screen.value("cursor").toList();
    if (cursor.size() == 2 && m_pane.value("focused").toBool()) {
        const int row = cursor[0].toInt(), col = cursor[1].toInt();
        p->setPen(QColor(bridge->frame().value("accent").toString()));
        if (editor)
            p->drawLine(QPointF(1 + cursorX(row, col), 1 + row * ch),
                        QPointF(1 + cursorX(row, col), (row + 1) * ch));
        else
            p->drawRect(1 + col * cw, 1 + row * ch, cw - 1, ch - 1);
    }
}
static QString keyName(QKeyEvent *e) {
    switch (e->key()) {
    case Qt::Key_Return:
    case Qt::Key_Enter:
        return "Enter";
    case Qt::Key_Backspace:
        return "Backspace";
    case Qt::Key_Delete:
        return "Delete";
    case Qt::Key_Insert:
        return "Insert";
    case Qt::Key_Up:
        return "Up";
    case Qt::Key_Down:
        return "Down";
    case Qt::Key_Left:
        return "Left";
    case Qt::Key_Right:
        return "Right";
    case Qt::Key_Home:
        return "Home";
    case Qt::Key_End:
        return "End";
    case Qt::Key_PageUp:
        return "PageUp";
    case Qt::Key_PageDown:
        return "PageDown";
    case Qt::Key_Tab:
    case Qt::Key_Backtab:
        return "Tab";
    case Qt::Key_Escape:
        return "Escape";
    default:
        if (e->key() >= Qt::Key_F1 && e->key() <= Qt::Key_F12)
            return QString("F%1").arg(e->key() - Qt::Key_F1 + 1);
        if (e->key() >= Qt::Key_A && e->key() <= Qt::Key_Z)
            return QChar(e->key()).toLower();
        // Control punctuation can have empty event text (e.g. Ctrl+,).
        if (e->key() >= Qt::Key_Space && e->key() <= Qt::Key_AsciiTilde)
            return QChar(e->key());
        return e->text();
    }
}
void Bridge::key(int code, const QString &text, int modifiers) {
    QKeyEvent event(QEvent::KeyPress, code, Qt::KeyboardModifiers(modifiers), text);
    const bool ctrl = event.modifiers().testFlag(Qt::ControlModifier),
               alt = event.modifiers().testFlag(Qt::AltModifier);
    const auto clipboard = QGuiApplication::clipboard()->text();
    if (ctrl || alt)
        send({{"action", "set_clipboard"}, {"text", clipboard}});
    const auto response =
        send({{"action", "key"},
              {"key", keyName(&event)},
              {"text", text},
              {"ctrl", ctrl},
              {"alt", alt},
              {"shift", event.modifiers().testFlag(Qt::ShiftModifier) || code == Qt::Key_Backtab}});
    if ((ctrl || alt) && response.value("clipboard").toString() != clipboard)
        QGuiApplication::clipboard()->setText(response.value("clipboard").toString());
    refresh();
}
void CellView::keyPressEvent(QKeyEvent *e) {
    if (bridge)
        bridge->key(e->key(), e->text(), int(e->modifiers()));
    e->accept();
}
void CellView::click(QMouseEvent *e, const QString &kind) {
    forceActiveFocus();
    if (!m_preedit.isEmpty())
        QGuiApplication::inputMethod()->commit();
    const int row = qMax(0, int(e->position().y() - 1) / bridge->cellHeight());
    int column = qMax(0, int(e->position().x() - 1) / bridge->cellWidth());
    if (m_pane.value("kind") == "editor") {
        layoutText();
        if (row < int(m_lines.size())) {
            const int position =
                m_lines[row]->lineAt(0).xToCursor(qMax(0.0, e->position().x() - 1));
            column = m_columns[row].value(position, column);
        }
    }
    const auto button =
        e->button() == Qt::MiddleButton || e->buttons().testFlag(Qt::MiddleButton) ? 1
        : e->button() == Qt::RightButton || e->buttons().testFlag(Qt::RightButton) ? 2
        : e->button() == Qt::LeftButton || e->buttons().testFlag(Qt::LeftButton)   ? 0
                                                                                   : 3;
    bridge->send({{"action", "pointer"},
                  {"pane", m_pane.value("id")},
                  {"row", row},
                  {"col", column},
                  {"kind", kind},
                  {"button", button},
                  {"shift", e->modifiers().testFlag(Qt::ShiftModifier)},
                  {"ctrl", e->modifiers().testFlag(Qt::ControlModifier)},
                  {"alt", e->modifiers().testFlag(Qt::AltModifier)}});
    bridge->refresh();
}
void CellView::mousePressEvent(QMouseEvent *e) {
    click(e, "press");
    e->accept();
}
void CellView::mouseMoveEvent(QMouseEvent *e) {
    click(e, e->buttons() == Qt::NoButton ? "move" : "drag");
    e->accept();
}
void CellView::mouseReleaseEvent(QMouseEvent *e) {
    click(e, "release");
    e->accept();
}
void CellView::hoverMoveEvent(QHoverEvent *e) {
    if (!bridge || m_pane.value("kind") != "terminal" ||
        !m_pane.value("terminal_mouse_motion").toBool())
        return;
    bridge->send({{"action", "pointer"},
                  {"pane", m_pane.value("id")},
                  {"row", qMax(0, int(e->position().y() - 1) / bridge->cellHeight())},
                  {"col", qMax(0, int(e->position().x() - 1) / bridge->cellWidth())},
                  {"kind", "move"},
                  {"button", 3},
                  {"shift", e->modifiers().testFlag(Qt::ShiftModifier)},
                  {"ctrl", e->modifiers().testFlag(Qt::ControlModifier)},
                  {"alt", e->modifiers().testFlag(Qt::AltModifier)}});
    e->accept();
}
void CellView::wheelEvent(QWheelEvent *e) {
    if (m_pane.value("kind") == "terminal")
        bridge->send({{"action", "pointer"},
                      {"pane", m_pane.value("id")},
                      {"row", qMax(0, int(e->position().y() - 1) / bridge->cellHeight())},
                      {"col", qMax(0, int(e->position().x() - 1) / bridge->cellWidth())},
                      {"kind", e->angleDelta().y() > 0 ? "wheel_up" : "wheel_down"},
                      {"shift", e->modifiers().testFlag(Qt::ShiftModifier)},
                      {"ctrl", e->modifiers().testFlag(Qt::ControlModifier)},
                      {"alt", e->modifiers().testFlag(Qt::AltModifier)}});
    else
        bridge->send({{"action", "scroll"},
                      {"pane", m_pane.value("id")},
                      {"delta", e->angleDelta().y() / 40}});
    bridge->refresh();
    e->accept();
}
void CellView::inputMethodEvent(QInputMethodEvent *e) {
    m_preedit = e->preeditString();
    m_layoutDirty = true;
    if (!e->commitString().isEmpty() || e->replacementLength() != 0) {
        if (m_pane.value("kind") == "editor")
            bridge->send({{"action", "input_method"},
                          {"text", e->commitString()},
                          {"replace_start", e->replacementStart()},
                          {"replace_length", e->replacementLength()}});
        else
            bridge->send({{"action", "paste"}, {"text", e->commitString()}});
        bridge->refresh();
    }
    update();
    e->accept();
}
QVariant CellView::inputMethodQuery(Qt::InputMethodQuery query) const {
    if (!bridge)
        return {};
    const auto editor = m_pane.value("editor").toMap();
    if (query == Qt::ImEnabled)
        return !m_pane.value("read_only").toBool();
    if (query == Qt::ImSurroundingText)
        return editor.value("surrounding");
    if (query == Qt::ImCursorPosition)
        return editor.value("cursor");
    if (query == Qt::ImAnchorPosition)
        return editor.value("anchor");
    if (query == Qt::ImCurrentSelection)
        return editor.value("selection");
    if (query == Qt::ImCursorRectangle) {
        const auto cursor = m_pane.value("screen").toMap().value("cursor").toList();
        if (cursor.size() == 2) {
            const int row = cursor[0].toInt(), col = cursor[1].toInt();
            return QRectF(1 + (m_pane.value("kind") == "editor" ? cursorX(row, col)
                                                                : col * bridge->cellWidth()),
                          1 + row * bridge->cellHeight(), bridge->cellWidth(),
                          bridge->cellHeight());
        }
    }
    return QQuickPaintedItem::inputMethodQuery(query);
}
static void initializeResources() { Q_INIT_RESOURCE(resources); }
extern "C" int slate_qt_run(void *context) {
    int argc = 1;
    char name[] = "slate-gui";
    char *argv[] = {name, nullptr};
#ifdef SLATE_SMOKE_TEST
    const bool tracing = !qEnvironmentVariableIsEmpty("SLATE_GUI_SMOKE_DIR");
    if (tracing)
        std::fprintf(stderr, "Smoke startup: constructing QApplication\n");
#endif
    QApplication app(argc, argv);
#ifdef SLATE_SMOKE_TEST
    if (tracing)
        std::fprintf(stderr, "Smoke startup: QApplication ready\n");
#endif
    app.setApplicationName("Slate");
    app.setOrganizationName("Slate");
    initializeResources();
    Bridge state(context);
    bridge = &state;
    qmlRegisterType<CellView>("Slate.Native", 1, 0, "CellView");
    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty("slate", &state);
#ifdef SLATE_SMOKE_TEST
    if (tracing)
        std::fprintf(stderr, "Smoke startup: loading QML\n");
#endif
    engine.load(QUrl("qrc:/slate/Main.qml"));
#ifdef SLATE_SMOKE_TEST
    if (tracing)
        std::fprintf(stderr, "Smoke startup: QML loaded\n");
#endif
    if (engine.rootObjects().isEmpty()) {
        bridge = nullptr;
        return 1;
    }
    QTimer timer;
    QObject::connect(&timer, &QTimer::timeout, &state, &Bridge::refresh);
    timer.start(1000); // Checkpoints and a fallback for non-Unix platforms.
    state.applyTheme();
    QObject::connect(&app, &QGuiApplication::paletteChanged, &state, [&state]() {
        state.applyTheme();
        state.refresh();
    });
    const int eventFd = slate_event_fd(context);
    QSocketNotifier *notifier =
        eventFd >= 0 ? new QSocketNotifier(eventFd, QSocketNotifier::Read, &state) : nullptr;
    if (notifier) {
        QObject::connect(notifier, &QSocketNotifier::activated, &state, [notifier, &state]() {
            notifier->setEnabled(false);
            QTimer::singleShot(16, &state, [notifier, &state]() {
                state.refresh();
                notifier->setEnabled(true);
            });
        });
    }
    state.refresh();

#ifdef SLATE_SMOKE_TEST
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_SMOKE_DIR"))
        startSmoke(&state, qobject_cast<QQuickWindow *>(engine.rootObjects().first()));
#endif
#ifdef SLATE_SMOKE_TEST
    if (tracing)
        std::fprintf(stderr, "Smoke startup: entering event loop\n");
#endif
    const int result = app.exec();
    bridge = nullptr;
    return result;
}
