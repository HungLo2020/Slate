#include "bridge.h"
#include <QApplication>
#include <QClipboard>
#include <QFileDialog>
#include <QFileInfo>
#include <QFontDatabase>
#include <QFontMetrics>
#include <QGuiApplication>
#include <QHoverEvent>
#include <QInputMethod>
#include <QInputMethodEvent>
#include <QIcon>
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
#include <QScopedValueRollback>
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
Bridge::Bridge(void *context, QObject *parent) : QObject(parent), m_context(context) {
    m_refreshTimer.setSingleShot(true);
    m_refreshTimer.setInterval(8);
    m_refreshTimer.setTimerType(Qt::PreciseTimer);
    connect(&m_refreshTimer, &QTimer::timeout, this, &Bridge::refresh);
}
Bridge::~Bridge() { delete m_pathDialog.data(); }
void Bridge::pickPath(const QString &kind, QObject *windowObject) {
    auto window = qobject_cast<QWindow *>(windowObject);
    if (!window || (kind != "open" && kind != "open-folder" && kind != "save-as"))
        return;
    if (m_pathDialog) {
        m_pathDialog->raise();
        m_pathDialog->activateWindow();
        return;
    }
    const auto context = send({{"action", "file_dialog_context"}});
    const auto path = context.value("path").toString();
    auto dialog = new QFileDialog;
    // QApplication + the desktop's platform theme supplies KDE's file dialog
    // on Plasma. Leave native dialogs and overwrite confirmation enabled.
    m_pathDialog = dialog;
    dialog->setObjectName("pathDialog");
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->setSupportedSchemes({"file"}); // Rust's document I/O uses local paths.
    dialog->setWindowTitle(kind == "save-as" ? tr("Save As") :
                           kind == "open-folder" ? tr("Open Folder") : tr("Open File"));
    dialog->setAcceptMode(kind == "save-as" ? QFileDialog::AcceptSave : QFileDialog::AcceptOpen);
    dialog->setFileMode(kind == "save-as" ? QFileDialog::AnyFile :
                        kind == "open-folder" ? QFileDialog::Directory : QFileDialog::ExistingFile);
    if (kind == "open-folder")
        dialog->setOption(QFileDialog::ShowDirsOnly);
    else
        dialog->setNameFilter(tr("All files (*)"));
    const auto directory = !path.isEmpty() && kind != "open-folder"
        ? QFileInfo(path).absolutePath() : m_frame.value("browser").toString();
    dialog->setDirectory(directory.isEmpty() ? context.value("root").toString() : directory);
    if (kind == "save-as" && !path.isEmpty())
        dialog->selectFile(path);
    dialog->setWindowModality(Qt::WindowModal);
    dialog->winId();
    dialog->windowHandle()->setTransientParent(window);
    connect(window, &QObject::destroyed, dialog, &QWidget::close);
    connect(dialog, &QFileDialog::fileSelected, this, [this, kind](const QString &selected) {
        if (selected.isEmpty()) return;
        if (kind == "open-folder") send({{"action", "show_workspace"}});
        send({{"action", kind == "save-as" ? "save_as" : "open"},
              {"path", selected}, {"overwrite", kind == "save-as"}});
    });
    connect(dialog, &QDialog::finished, this, [this, parent = QPointer<QWindow>(window)](int) {
        m_pathDialog.clear();
        if (parent) parent->requestActivate();
        refresh();
        emit pathDialogOpenChanged();
    });
    // Consume the core prompt before showing the native dialog. Cancellation
    // leaves the document untouched, and paths bypass command-line parsing.
    send({{"action", "dismiss_prompt"}});
    emit pathDialogOpenChanged();
    refresh();
    dialog->open(); // Asynchronous: workers and Qt's event loop keep running.
}
void Bridge::scheduleRefresh() {
    if (!m_refreshTimer.isActive()) m_refreshTimer.start();
}
QVariantMap Bridge::diagnostics() const {
    return {{"updates", m_updates}, {"last_update_bytes", m_lastBytes},
            {"clipboard_reads", m_clipboardReads}};
}
void Bridge::attachView(int id, CellView *view) {
    m_views.insert(id, view);
    for (const auto &p : m_frame.value("panes").toList()) {
        if (p.toMap().value("id").toInt() == id) { view->setPane(p.toMap()); break; }
    }
    if (m_surfaces.contains(id)) view->applySurface(m_surfaces.value(id));
}
void Bridge::detachView(int id, CellView *view) {
    if (m_views.value(id) == view) m_views.remove(id);
}
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
    auto exchange = [this](const QVariantMap &request) {
        const auto bytes = QJsonDocument(QJsonObject::fromVariantMap(request)).toJson(QJsonDocument::Compact);
        char *raw = slate_request(m_context, bytes.constData());
        const QByteArray response(raw);
        if (request.value("action") == "update") m_lastBytes = response.size();
        const auto result = QJsonDocument::fromJson(response).object().toVariantMap();
        slate_response_free(raw);
        return result;
    };
    auto result = exchange(command);
    if (result.value("clipboard_request").toBool()) {
        ++m_clipboardReads;
        auto retry = command;
        retry.insert("clipboard_input", QGuiApplication::clipboard()->text());
        result = exchange(retry);
    }
    if (result.contains("clipboard"))
        QGuiApplication::clipboard()->setText(result.value("clipboard").toString());
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
    scheduleRefresh();
}
void Bridge::paneHeader(int height) {
    const int next = qBound(1, height, 65535);
    if (next == m_headerHeight) return;
    m_headerHeight = next;
    scheduleRefresh();
}
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
static QVariantMap patchTerminal(QVariantMap prior, const QVariantMap &patch) {
    auto cells = prior.value("cells").toList();
    const int rows = patch.value("rows").toInt();
    while (cells.size() > rows) cells.removeLast();
    while (cells.size() < rows) cells.append(QVariant(QVariantList()));
    for (const auto &entry : patch.value("lines").toList()) {
        const auto line = entry.toMap();
        const int row = line.value("row").toInt();
        if (row >= 0 && row < cells.size()) cells[row] = line.value("cells");
    }
    // Complete cached surfaces are used when a pane delegate is recreated.
    if (patch.contains("cells")) cells = patch.value("cells").toList();
    prior = patch;
    prior.remove("lines");
    prior.insert("cells", cells);
    return prior;
}
// Assemble compact native content separately from the small metadata QML sees.
static QVariantMap patchSurface(QVariantMap prior, const QVariantMap &patch) {
    if (prior.value("kind") != patch.value("kind")) prior.clear();
    if (patch.value("kind") == "editor") {
        auto lines = prior.value("lines").toList();
        const int count = patch.value("line_count").toInt();
        while (lines.size() > count) lines.removeLast();
        while (lines.size() < count) lines.append(QVariantMap{{"row", lines.size()}});
        for (const auto &entry : patch.value("lines").toList()) {
            const auto change = entry.toMap();
            const int row = change.value("row").toInt();
            if (row < 0 || row >= lines.size()) continue;
            auto data = lines[row].toMap();
            for (auto it = change.begin(); it != change.end(); ++it) data.insert(it.key(), it.value());
            lines[row] = data;
        }
        prior.insert("lines", lines);
    }
    for (auto it = patch.begin(); it != patch.end(); ++it)
        if (it.key() == "screen") prior.insert("screen", patchTerminal(prior.value("screen").toMap(), it.value().toMap()));
        else if (it.key() != "lines") prior.insert(it.key(), it.value());
    return prior;
}
void Bridge::refresh() {
    if (m_refreshing) { scheduleRefresh(); return; }
    QScopedValueRollback<bool> refreshing(m_refreshing, true);
    m_refreshTimer.stop();
    auto frame = send({{"action", "update"}, {"width", m_width}, {"height", m_height},
                       {"cell_width", cellWidth()}, {"cell_height", cellHeight()},
                       {"header_height", m_headerHeight}, {"minimum_width", qMax(160, m_headerHeight * 4)}});
    if (frame.value("unchanged").toBool()) { emit refreshFinished(); return; }
    ++m_updates;
    const auto surfaces = frame.take("surfaces").toList();
    for (const auto &key : {"files", "git", "settings"})
        if (!frame.contains(key)) frame.insert(key, m_frame.value(key));
    QVariantList panes, handles;
    for (const auto &p : frame.value("panes").toList()) {
        const auto pane = p.toMap();
        const int id = pane.value("id").toInt();
        panes.append(pane.value("id"));
        if (m_surfaces.value(id).value("kind") != pane.value("kind")) m_surfaces.remove(id);
    }
    for (auto it = m_surfaces.begin(); it != m_surfaces.end();) {
        if (!panes.contains(it.key())) it = m_surfaces.erase(it); else ++it;
    }
    for (const auto &h : frame.value("handles").toList()) handles.append(h.toMap().value("id"));
    const bool structure = panes != m_paneIds || handles != m_handleIds;
    const bool fileChange = frame.value("files") != m_frame.value("files"),
               gitChange = frame.value("git") != m_frame.value("git");
    m_frame = frame;
    m_paneIds = panes;
    m_handleIds = handles;
    // Store before delegates are created, so a restored/recreated view can
    // immediately attach to complete native content even on a metadata-only frame.
    for (const auto &entry : surfaces) {
        const auto patch = entry.toMap();
        const int id = patch.value("id").toInt();
        m_surfaces.insert(id, patchSurface(m_surfaces.value(id), patch));
        if (auto view = m_views.value(id)) view->applySurface(patch);
    }
    if (structure) emit structureChanged();
    for (const auto &p : frame.value("panes").toList()) {
        const auto pane = p.toMap();
        if (auto view = m_views.value(pane.value("id").toInt())) view->setPane(pane);
    }
    if (fileChange) { m_files.replace(m_frame.value("files").toList()); emit filesChanged(); }
    if (gitChange) { m_git.replace(m_frame.value("git").toList()); emit gitChanged(); }
    emit frameChanged();
    emit refreshFinished();
    if (frame.value("quit").toBool()) QCoreApplication::quit();
}
void Bridge::copyClipboard() { send({{"action", "copy"}}); scheduleRefresh(); }
void Bridge::pasteClipboard() { send({{"action", "paste"}}); scheduleRefresh(); }
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
CellView::~CellView() { if (bridge) bridge->detachView(m_paneId, this); }
void CellView::setPaneId(int id) {
    if (m_paneId == id) return;
    if (bridge) bridge->detachView(m_paneId, this);
    m_paneId = id;
    if (bridge) bridge->attachView(id, this);
    emit paneIdChanged();
}
void CellView::setPane(const QVariantMap &pane) {
    if (m_pane == pane) return;
    const bool kindChanged = m_pane.value("kind") != pane.value("kind");
    m_pane = pane;
    if (kindChanged && pane.value("kind") != "editor") {
        m_lines.clear(); m_columns.clear(); m_lineData.clear(); m_overlays.clear();
        m_layoutPreedit.clear(); m_preeditPosition.clear();
    }
    if (hasActiveFocus() && QGuiApplication::inputMethod()) QGuiApplication::inputMethod()->update(Qt::ImQueryAll);
    emit paneChanged();
    update();
}
void CellView::applySurface(const QVariantMap &patch) {
    m_cursor = patch.value("cursor").toList();
    if (patch.value("kind") == "terminal") {
        if (patch.contains("screen")) m_screen = patchTerminal(m_screen, patch.value("screen").toMap());
        update();
        return;
    }
    const int count = patch.value("line_count").toInt();
    m_lines.resize(count); m_columns.resize(count);
    m_lineData.resize(count); m_overlays.resize(count);
    m_layoutPreedit.resize(count); m_preeditPosition.resize(count);
    for (const auto &entry : patch.value("lines").toList()) {
        const auto line = entry.toMap();
        const int row = line.value("row").toInt();
        if (row < 0 || row >= count) continue;
        if (line.contains("layout")) {
            const auto data = line.value("layout").toMap();
            if (data != m_lineData[row]) { m_lineData[row] = data; m_lines[row].reset(); }
        }
        if (line.contains("overlays")) m_overlays[row] = line.value("overlays").toList();
    }
    if (hasActiveFocus() && QGuiApplication::inputMethod())
        QGuiApplication::inputMethod()->update(Qt::ImCursorRectangle);
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
    if (!m_lines[row] || m_lines[row]->lineCount() == 0) return column * bridge->cellWidth();
    const auto &columns = m_columns[row];
    const int end = columns.isEmpty() ? 0 : columns.last();
    return m_lines[row]->lineAt(0).cursorToX(textPosition(row, column)) +
           qMax(0, column - end) * bridge->cellWidth();
}
static QTextLayout::FormatRange textFormat(const QVariantMap &span, bool overlay = false) {
    QTextCharFormat format;
    format.setForeground(QColor(span.value("fg").toString()));
    format.setBackground(QColor(span.value("bg").toString()));
    if (!overlay) {
        format.setFontWeight(span.value("bold").toBool() ? QFont::Bold : QFont::Normal);
        format.setFontItalic(span.value("italic").toBool());
    }
    format.setFontUnderline(span.value("underline").toBool());
    return {span.value("start").toInt(), span.value("length").toInt(), format};
}
void CellView::layoutText() const {
    if (!bridge || m_pane.value("kind") != "editor") return;
    for (int row = 0; row < int(m_lines.size()); ++row) {
        const bool composing = m_cursor.size() == 2 && m_cursor[0].toInt() == row && !m_preedit.isEmpty();
        const QString preedit = composing ? m_preedit : QString();
        const int position = composing ? m_cursor[1].toInt() : -1;
        if (m_lines[row] && m_layoutPreedit[row] == preedit && m_preeditPosition[row] == position) continue;
        const auto data = m_lineData[row];
        QVector<int> columns;
        for (const auto &col : data.value("columns").toList()) columns.append(col.toInt());
        m_columns[row] = columns;
        auto layout = std::make_unique<QTextLayout>(data.value("text").toString(), bridge->font());
        QTextOption option;
        option.setWrapMode(QTextOption::NoWrap);
        option.setTextDirection(Qt::LeftToRight);
        layout->setTextOption(option);
        QList<QTextLayout::FormatRange> formats;
        for (const auto &span : data.value("formats").toList()) formats.append(textFormat(span.toMap()));
        layout->setFormats(formats);
        layout->setCacheEnabled(true);
        if (composing) layout->setPreeditArea(textPosition(row, position), preedit);
        layout->beginLayout();
        auto line = layout->createLine();
        if (line.isValid()) line.setLineWidth(1000000);
        layout->endLayout();
        m_lines[row] = std::move(layout);
        m_layoutPreedit[row] = preedit;
        m_preeditPosition[row] = position;
        ++m_layoutBuilds;
    }
}
void CellView::paint(QPainter *p) {
    if (!bridge)
        return;
    const bool editor = m_pane.value("kind") == "editor";
    p->fillRect(boundingRect(), editor ? QColor(bridge->frame().value("background").toString())
                                       : QColor("#20242c"));
    const auto screen = m_screen;
    const int cw = bridge->cellWidth(), ch = bridge->cellHeight();
    if (editor) {
        layoutText();
        for (int row = 0; row < int(m_lines.size()); ++row) {
            QList<QTextLayout::FormatRange> overlays;
            for (const auto &span : m_overlays[row]) overlays.append(textFormat(span.toMap(), true));
            m_lines[row]->draw(p, QPointF(1, 1 + row * ch), overlays);
        }
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
    const auto cursor = m_cursor;
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
    send({{"action", "key"}, {"key", keyName(&event)}, {"text", text},
          {"ctrl", ctrl}, {"alt", alt},
          {"shift", event.modifiers().testFlag(Qt::ShiftModifier) || code == Qt::Key_Backtab}});
    scheduleRefresh();
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
        if (row < int(m_lines.size()) && m_lines[row] && m_lines[row]->lineCount() > 0) {
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
    bridge->scheduleRefresh();
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
    bridge->scheduleRefresh();
    e->accept();
}
void CellView::inputMethodEvent(QInputMethodEvent *e) {
    m_preedit = e->preeditString();
    if (!e->commitString().isEmpty() || e->replacementLength() != 0) {
        if (m_pane.value("kind") == "editor")
            bridge->send({{"action", "input_method"},
                          {"text", e->commitString()},
                          {"replace_start", e->replacementStart()},
                          {"replace_length", e->replacementLength()}});
        else
            bridge->send({{"action", "paste"}, {"text", e->commitString()}});
        bridge->scheduleRefresh();
    }
    update();
    e->accept();
}
QVariant CellView::inputMethodQuery(Qt::InputMethodQuery query) const {
    if (!bridge)
        return {};
    // Input is applied immediately but painting is coalesced. IME queries must
    // observe the current document even before the next presentation update.
    const bool textQuery = query == Qt::ImSurroundingText || query == Qt::ImCursorPosition ||
                           query == Qt::ImAnchorPosition || query == Qt::ImCurrentSelection;
    const auto editor = textQuery && m_pane.value("kind") == "editor"
        ? bridge->send({{"action", "input_context"}, {"pane", m_paneId}}).value("editor").toMap()
        : m_pane.value("editor").toMap();
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
        const auto cursor = m_cursor;
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
    app.setDesktopFileName("slate");
    initializeResources();
    app.setWindowIcon(QIcon(":/slate/slate.svg"));
    Bridge state(context);
    bridge = &state;
    qmlRegisterType<CellView>("Slate.Native", 1, 0, "CellView");
    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty("slate", &state);
    state.applyTheme();
    state.refresh();
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
            state.scheduleRefresh();
        });
    }
    if (notifier) QObject::connect(&state, &Bridge::refreshFinished, notifier, [notifier]() { notifier->setEnabled(true); });
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
