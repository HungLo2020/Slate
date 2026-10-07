#include "bridge.h"
#include <QAccessible>
#include <QAccessibleObject>
#include <QApplication>
#include <QClipboard>
#include <QDir>
#include <QFile>
#include <QFileDialog>
#include <QFileInfo>
#include <QFontDatabase>
#include <QFontMetricsF>
#include <QGuiApplication>
#include <QHoverEvent>
#include <QIcon>
#include <QInputMethod>
#include <QInputMethodEvent>
#include <QJsonDocument>
#include <QJsonObject>
#include <QKeyEvent>
#include <QMessageBox>
#include <QMouseEvent>
#include <QPainter>
#include <QPalette>
#include <QPrintDialog>
#include <QPrinter>
#include <QProcess>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickWindow>
#include <QScopedValueRollback>
#include <QSessionManager>
#include <QSocketNotifier>
#include <QStyleHints>
#include <QTextCharFormat>
#include <QTextDocument>
#include <QTextLayout>
#include <QTimer>
#include <QWheelEvent>
#include <cmath>
#include <cstdio>
extern "C" int slate_event_fd(void *);
extern "C" char *slate_request(void *, const char *);
extern "C" void slate_response_free(char *);
extern "C" void slate_set_waker(void *, void (*)(void *), void *);
static Bridge *bridge = nullptr;
#ifdef SLATE_SMOKE_TEST
void startSmoke(Bridge *, QQuickWindow *);
#endif

QPixmap IconProvider::requestPixmap(const QString &id, QSize *size, const QSize &requested) {
    const auto name = id.section('?', 0, 0);
    const auto tint = id.section('?', 1);
    const QSize extent = requested.isValid() && !requested.isEmpty() ? requested : QSize(16, 16);
    if (size) *size = extent;
    const auto icon = QIcon::fromTheme(name);
    if (icon.isNull()) return {};
    auto pixmap = icon.pixmap(extent);
    if (QColor color(tint); color.isValid() && !pixmap.isNull()) {
        QPainter painter(&pixmap);
        painter.setCompositionMode(QPainter::CompositionMode_SourceIn);
        painter.fillRect(pixmap.rect(), color);
    }
    return pixmap;
}

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
QStringList Bridge::encodings() const {
    return {"UTF-8", "UTF-16LE", "UTF-16BE", "windows-1252", "ISO-8859-15", "ISO-8859-2",
            "windows-1251", "KOI8-R", "Shift_JIS", "EUC-JP", "GBK", "Big5", "EUC-KR"};
}
QStringList Bridge::fontFamilies() const {
    // First entry: the desktop's fixed-width font. Fixed-pitch families follow.
    QStringList families{QString()};
    for (const auto &family : QFontDatabase::families())
        if (QFontDatabase::isFixedPitch(family) && !QFontDatabase::isPrivateFamily(family))
            families.append(family);
    return families;
}
bool Bridge::hasIcon(const QString &name) const { return QIcon::hasThemeIcon(name); }
QString Bridge::localPath(const QUrl &url) const {
    return url.isLocalFile() ? url.toLocalFile() : QString();
}
QVariantList Bridge::commands(const QString &query) {
    if (query.isEmpty()) {
        if (!m_catalogs.contains("0:-1"))
            m_catalogs.insert("0:-1", send({{"action", "catalog"}, {"query", query}}).value("commands").toList());
        return m_catalogs.value("0:-1");
    }
    return send({{"action", "catalog"}, {"query", query}}).value("commands").toList();
}
QVariantMap Bridge::commandInfo(const QString &id, int pane, int row) {
    QString kind;
    if (row >= 0)
        for (const auto &value : m_frame.value("panes").toList()) {
            const auto info = value.toMap();
            if (info.value("id").toInt() == pane) { kind = info.value("kind").toString(); break; }
        }
    const auto key = row >= 0 ? QString("%1:%2:%3").arg(pane).arg(row).arg(kind)
                             : QString("%1:%2").arg(pane).arg(row);
    auto &catalogs = row >= 0 ? m_rowCatalogs : m_catalogs;
    if (!catalogs.contains(key)) {
        QVariantMap request{{"action", "catalog"}};
        if (row >= 0) request.insert("query", "selected");
        if (pane) request.insert("pane", pane);
        if (row >= 0) request.insert("row", row);
        catalogs.insert(key, send(request).value("commands").toList());
    }
    for (const auto &value : catalogs.value(key)) {
        const auto info = value.toMap();
        if (info.value("id") == id) return info;
    }
    return {};
}
QVariantMap Bridge::overview(int pane) {
    return send({{"action", "overview"}, {"pane", pane}}).value("overview").toMap();
}
Bridge::Bridge(void *context, QObject *parent) : QObject(parent), m_context(context) {
    m_refreshTimer.setSingleShot(true);
    m_refreshTimer.setInterval(8);
    m_refreshTimer.setTimerType(Qt::PreciseTimer);
    connect(&m_refreshTimer, &QTimer::timeout, this, &Bridge::refresh);
    updateFont({});
    if (auto clipboard = QGuiApplication::clipboard()) {
        m_clipboard = clipboard->text();
        connect(clipboard, &QClipboard::dataChanged, this, [this]() {
            m_clipboard = QGuiApplication::clipboard()->text();
        });
    }
}
Bridge::~Bridge() {
    delete m_pathDialog.data();
    delete m_closeDialog.data();
}
void Bridge::updateFont(const QVariantMap &settings) {
    const auto family = settings.value("font_family").toString();
    const int size = qBound(6, settings.value("font_size", 11).toInt(), 72);
    const auto key = family + "/" + QString::number(size);
    if (key == m_fontKey) return;
    m_fontKey = key;
    QFont font = family.isEmpty() ? QFontDatabase::systemFont(QFontDatabase::FixedFont) : QFont(family);
    font.setPointSize(size);
    font.setStyleHint(QFont::Monospace);
    m_font = font;
    for (int i = 0; i < 4; ++i) {
        m_variants[i] = font;
        m_variants[i].setBold(i & 1);
        m_variants[i].setItalic(i & 2);
    }
    const QFontMetricsF metrics(font);
    m_cellWidth = int(std::ceil(metrics.horizontalAdvance('M')));
    m_cellHeight = int(std::ceil(metrics.height()));
    for (auto view : m_views) view->fontChanged();
    emit fontChanged();
    scheduleRefresh();
}
void Bridge::confirmCloseTab(QObject *windowObject) {
    auto window = qobject_cast<QWindow *>(windowObject);
    const auto prompt = m_frame.value("prompt").toMap();
    if (!window || m_closeDialog || prompt.value("kind") != "close-tab") return;
    auto dialog = new QMessageBox(QMessageBox::Warning, tr("Unsaved Changes"),
        tr("Discard unsaved changes in “%1” and close this tab?").arg(prompt.value("input").toString()),
        QMessageBox::Discard | QMessageBox::Cancel);
    m_closeDialog = dialog;
    dialog->setObjectName("closeTabDialog");
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->setTextFormat(Qt::PlainText);
    dialog->setInformativeText(tr("Cancel to keep editing or save the document first."));
    dialog->setDefaultButton(QMessageBox::Cancel);
    dialog->setEscapeButton(QMessageBox::Cancel);
    dialog->setWindowModality(Qt::WindowModal);
    dialog->winId();
    dialog->windowHandle()->setTransientParent(window);
    connect(window, &QObject::destroyed, dialog, &QWidget::close);
    connect(dialog, &QDialog::finished, this, [this, parent = QPointer<QWindow>(window)](int result) {
        m_closeDialog.clear();
        if (result == QMessageBox::Discard)
            send({{"action", "submit_prompt"}, {"all", true}});
        else
            send({{"action", "dismiss_prompt"}});
        if (parent) parent->requestActivate();
        refresh();
        emit closeDialogOpenChanged();
    });
    emit closeDialogOpenChanged();
    dialog->open();
}
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
    // Open accepts several files at once, like other desktop editors.
    dialog->setFileMode(kind == "save-as" ? QFileDialog::AnyFile :
                        kind == "open-folder" ? QFileDialog::Directory : QFileDialog::ExistingFiles);
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
    connect(dialog, &QFileDialog::filesSelected, this, [this, kind](const QStringList &selected) {
        if (selected.isEmpty()) return;
        if (kind == "open-folder") send({{"action", "show_workspace"}});
        for (const auto &file : selected)
            send({{"action", kind == "save-as" ? "save_as" : "open"},
                  {"path", file}, {"overwrite", kind == "save-as"}});
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
void Bridge::print() {
    const auto document = send({{"action", "document_text"}});
    if (!document.contains("text")) return;
    auto text = std::make_shared<QTextDocument>();
    text->setDefaultFont(m_font);
    text->setPlainText(document.value("text").toString());
    text->setMetaInformation(QTextDocument::DocumentTitle, document.value("title").toString());
    // Tests (and scripted use) print straight to a PDF file.
    const auto pdf = qEnvironmentVariable("SLATE_GUI_PRINT_PDF");
    if (!pdf.isEmpty()) {
        QPrinter printer(QPrinter::HighResolution);
        printer.setOutputFormat(QPrinter::PdfFormat);
        printer.setOutputFileName(pdf);
        printer.setDocName(document.value("title").toString());
        text->print(&printer);
        return;
    }
    auto printer = std::make_shared<QPrinter>(QPrinter::HighResolution);
    printer->setDocName(document.value("title").toString());
    auto dialog = new QPrintDialog(printer.get());
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->setWindowTitle(tr("Print %1").arg(document.value("title").toString()));
    connect(dialog, &QDialog::accepted, this, [printer, text]() { text->print(printer.get()); });
    dialog->open();
}
void Bridge::performRequests(const QVariantList &requests) {
    for (const auto &request : requests) {
        const auto name = request.toString();
        if (name == "new-window") {
            const auto log = qEnvironmentVariable("SLATE_GUI_NEW_WINDOW_LOG");
            if (!log.isEmpty()) {
                // Scripted tests record the request instead of opening a window.
                QFile file(log);
                if (file.open(QIODevice::Append)) file.write("new-window\n");
                continue;
            }
            QProcess::startDetached(QCoreApplication::applicationFilePath(), {"--new-instance"},
                                    m_frame.value("browser").toString());
        } else if (name == "print") {
            QTimer::singleShot(0, this, &Bridge::print);
        } else if (name == "raise" && m_window) {
            m_window->show();
            m_window->raise();
            m_window->requestActivate();
        }
    }
}
void Bridge::scheduleRefresh() {
    if (!m_refreshTimer.isActive()) m_refreshTimer.start();
}
QVariantMap Bridge::diagnostics() const {
    return {{"updates", m_updates}, {"last_update_bytes", m_lastBytes},
            {"clipboard_reads", m_clipboardReads}, {"catalog_requests", m_catalogRequests}};
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
QVariantMap Bridge::send(const QVariantMap &command) {
    const auto action = command.value("action").toString();
    if (action == "catalog") ++m_catalogRequests;
    if (action != "catalog" && action != "input_context" && action != "file_dialog_context" &&
        action != "diagnostics" && action != "update" && action != "snapshot" && action != "overview" &&
        action != "document_text")
        m_catalogs.clear();
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
        retry.insert("clipboard_input", m_clipboard);
        result = exchange(retry);
    }
    if (result.contains("clipboard")) {
        m_clipboard = result.value("clipboard").toString();
        QGuiApplication::clipboard()->setText(m_clipboard);
    }
    if (result.contains("command_revision") && result.value("command_revision") != m_catalogRevision) {
        m_catalogs.clear();
        m_catalogRevision = result.value("command_revision");
    }
    // Row action state depends on Git, not cursor movement in another pane.
    if (result.contains("command_revision") &&
        (result.value("git_revision") != m_frame.value("git_revision") ||
         result.value("git_busy") != m_frame.value("git_busy") || result.contains("global_shortcuts")))
        m_rowCatalogs.clear();
    if (result.contains("confirmation"))
        emit confirmationRequested(result.value("confirmation").toString());
    if (result.value("apply_theme").toBool())
        applyTheme();
    if (result.contains("requests"))
        performRequests(result.value("requests").toList());
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
                       {"cell_width", m_cellWidth}, {"cell_height", m_cellHeight},
                       {"header_height", m_headerHeight}, {"minimum_width", qMax(160, m_headerHeight * 4)}});
    if (frame.value("unchanged").toBool()) { emit refreshFinished(); return; }
    ++m_updates;
    const auto surfaces = frame.take("surfaces").toList();
    for (const auto &key : {"files", "git", "settings", "global_shortcuts"})
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
    updateFont(frame.value("settings").toMap());
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

// ---------------------------------------------------------------------------
// Editor accessibility: screen readers read the document around the caret,
// follow it, and get character positions on screen.
class EditorAccessible : public QAccessibleObject, public QAccessibleTextInterface {
  public:
    explicit EditorAccessible(CellView *view) : QAccessibleObject(view) {}
    CellView *view() const { return static_cast<CellView *>(object()); }
    QAccessibleInterface *parent() const override {
        auto item = view()->parentItem();
        return item ? QAccessible::queryAccessibleInterface(item) : nullptr;
    }
    int childCount() const override { return 0; }
    QAccessibleInterface *child(int) const override { return nullptr; }
    int indexOfChild(const QAccessibleInterface *) const override { return -1; }
    QAccessibleInterface *childAt(int, int) const override { return nullptr; }
    QAccessible::Role role() const override {
        return view()->isEditor() ? QAccessible::EditableText : QAccessible::Terminal;
    }
    QAccessible::State state() const override {
        QAccessible::State s;
        s.focusable = true;
        s.focused = view()->hasActiveFocus();
        s.editable = view()->isEditor() && !view()->pane().value("read_only").toBool();
        s.multiLine = true;
        s.readOnly = !s.editable;
        s.invisible = !view()->isVisible();
        return s;
    }
    QString text(QAccessible::Text t) const override {
        if (t == QAccessible::Name) {
            for (const auto &tab : view()->pane().value("tabs").toList())
                if (tab.toMap().value("active").toBool())
                    return tab.toMap().value("title").toString();
            return view()->isEditor() ? QStringLiteral("Editor") : QStringLiteral("Terminal");
        }
        if (t == QAccessible::Value) return context().value("surrounding").toString();
        return {};
    }
    QRect rect() const override {
        auto window = view()->window();
        if (!window) return {};
        const auto scene = view()->mapRectToScene(view()->boundingRect()).toRect();
        return QRect(window->mapToGlobal(scene.topLeft()), scene.size());
    }
    void *interface_cast(QAccessible::InterfaceType type) override {
        if (type == QAccessible::TextInterface) return static_cast<QAccessibleTextInterface *>(this);
        return QAccessibleObject::interface_cast(type);
    }
    // QAccessibleTextInterface over the text window around the caret.
    QVariantMap context() const { return view()->accessibleContext(); }
    void selection(int index, int *start, int *end) const override {
        const auto c = context();
        const int a = c.value("anchor").toInt(), b = c.value("cursor").toInt();
        if (index == 0 && a != b) { *start = qMin(a, b); *end = qMax(a, b); }
        else *start = *end = 0;
    }
    int selectionCount() const override {
        const auto c = context();
        return c.value("anchor").toInt() != c.value("cursor").toInt() ? 1 : 0;
    }
    void addSelection(int, int) override {}
    void removeSelection(int) override {}
    void setSelection(int, int, int) override {}
    int cursorPosition() const override { return context().value("cursor").toInt(); }
    void setCursorPosition(int) override {}
    QString text(int start, int end) const override {
        return context().value("surrounding").toString().mid(start, qMax(0, end - start));
    }
    int characterCount() const override { return context().value("surrounding").toString().size(); }
    QRect characterRect(int offset) const override {
        if (offset != cursorPosition()) return {};
        auto window = view()->window();
        if (!window) return {};
        const auto scene = view()->mapRectToScene(view()->cursorRectangle()).toRect();
        return QRect(window->mapToGlobal(scene.topLeft()), scene.size());
    }
    int offsetAtPoint(const QPoint &) const override { return -1; }
    void scrollToSubstring(int, int) override {}
    QString attributes(int, int *start, int *end) const override {
        *start = *end = 0;
        return {};
    }
};
static QAccessibleInterface *accessibleFactory(const QString &className, QObject *object) {
    if (className == QLatin1String("CellView"))
        if (auto view = qobject_cast<CellView *>(object))
            return new EditorAccessible(view);
    return nullptr;
}

// ---------------------------------------------------------------------------
CellView::CellView(QQuickItem *parent) : QQuickPaintedItem(parent) {
    setAcceptedMouseButtons(Qt::LeftButton | Qt::MiddleButton | Qt::RightButton);
    setAcceptHoverEvents(true);
    setFlag(ItemAcceptsInputMethod, true);
    setActiveFocusOnTab(true);
    setClip(true);
    setOpaquePainting(true);
    m_blink.setInterval(qMax(200, QGuiApplication::styleHints()->cursorFlashTime() / 2));
    connect(&m_blink, &QTimer::timeout, this, [this]() {
        m_cursorShown = !m_cursorShown;
        update();
    });
}
CellView::~CellView() { if (bridge) bridge->detachView(m_paneId, this); }
void CellView::setPaneId(int id) {
    if (m_paneId == id) return;
    if (bridge) bridge->detachView(m_paneId, this);
    m_paneId = id;
    if (bridge) bridge->attachView(id, this);
    emit paneIdChanged();
}
void CellView::fontChanged() {
    m_lines.clear();
    m_lines.resize(m_lineData.size());
    m_scrollPixels = 0;
    update();
}
void CellView::restartBlink() {
    m_cursorShown = true;
    // A cursor flash time of 0 means the desktop disables blinking.
    if (hasActiveFocus() && QGuiApplication::styleHints()->cursorFlashTime() > 0)
        m_blink.start();
    else
        m_blink.stop();
}
void CellView::focusInEvent(QFocusEvent *event) {
    QQuickPaintedItem::focusInEvent(event);
    restartBlink();
    update();
}
void CellView::focusOutEvent(QFocusEvent *event) {
    QQuickPaintedItem::focusOutEvent(event);
    m_blink.stop();
    m_cursorShown = true;
    update();
}
void CellView::setPane(const QVariantMap &pane) {
    if (m_pane == pane) return;
    const bool kindChanged = m_pane.value("kind") != pane.value("kind");
    m_pane = pane;
    if (kindChanged && pane.value("kind") != "editor") {
        m_lines.clear(); m_columns.clear(); m_lineData.clear(); m_overlays.clear();
        m_layoutPreedit.clear(); m_preeditPosition.clear();
    }
    // A cursor-driven scroll (not the wheel) aligns rows to the top again.
    const int top = pane.value("editor").toMap().value("top", -1).toInt();
    if (top != m_lastTop) {
        if (m_requestedLines == 0 || top == m_lastTop) m_scrollPixels = 0;
        m_requestedLines = 0;
        m_lastTop = top;
    }
    if (hasActiveFocus() && QGuiApplication::inputMethod()) QGuiApplication::inputMethod()->update(Qt::ImQueryAll);
    emit paneChanged();
    update();
}
void CellView::notifyAccessibleCursor() {
    if (!QAccessible::isActive() || !isEditor()) return;
    QAccessibleTextCursorEvent event(this, accessibleContext().value("cursor").toInt());
    QAccessible::updateAccessibility(&event);
}
void CellView::applySurface(const QVariantMap &patch) {
    const auto previousCursor = m_cursor;
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
    if (m_cursor != previousCursor) {
        restartBlink();
        notifyAccessibleCursor();
    }
    update();
}
QVariantMap CellView::accessibleContext() const {
    if (!bridge || !isEditor()) return {};
    return bridge->send({{"action", "input_context"}, {"pane", m_paneId}}).value("editor").toMap();
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
QRectF CellView::cursorRectangle() const {
    if (m_cursor.size() != 2 || !bridge) return {};
    const int row = m_cursor[0].toInt(), col = m_cursor[1].toInt();
    const qreal x = 1 + (isEditor() ? cursorX(row, col) : col * bridge->cellWidth());
    return QRectF(x, 1 + row * bridge->cellHeight() - m_scrollPixels, bridge->cellWidth(), bridge->cellHeight());
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
        // Bidirectional text is shaped by Qt; columns map logical positions.
        option.setTextDirection(Qt::LeftToRight);
        layout->setTextOption(option);
        QList<QTextLayout::FormatRange> formats;
        for (const auto &span : data.value("formats").toList()) formats.append(textFormat(span.toMap()));
        if (composing) {
            const int at = textPosition(row, position);
            // The input method's own styling of the composition (underlines,
            // highlighted segment) applies on top of the document text.
            for (auto range : m_preeditFormats) {
                range.start += at;
                formats.append(range);
            }
            if (m_preeditFormats.isEmpty()) {
                QTextCharFormat underline;
                underline.setFontUnderline(true);
                formats.append({at, int(preedit.size()), underline});
            }
        }
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
    const QColor background(bridge->frame().value("background").toString());
    p->fillRect(boundingRect(), background);
    const auto screen = m_screen;
    const int cw = bridge->cellWidth(), ch = bridge->cellHeight();
    const qreal offset = editor ? m_scrollPixels : 0;
    if (editor) {
        layoutText();
        for (int row = 0; row < int(m_lines.size()); ++row) {
            if (!m_lines[row]) continue;
            const qreal y = 1 + row * ch - offset;
            if (y > height()) break;
            QList<QTextLayout::FormatRange> overlays;
            for (const auto &span : m_overlays[row]) overlays.append(textFormat(span.toMap(), true));
            m_lines[row]->draw(p, QPointF(1, y), overlays);
        }
    } else {
        // Runs of cells with the same style are drawn together; wide and
        // non-ASCII glyphs keep their own cell so columns never drift.
        const auto rows = screen.value("cells").toList();
        const auto ascent = QFontMetrics(bridge->font()).ascent();
        for (int row = 0; row < rows.size(); ++row) {
            const auto line = rows[row].toList();
            int col = 0;
            while (col < line.size()) {
                const auto cell = line[col].toMap();
                const auto fg = cell.value("fg").toString(), bg = cell.value("bg").toString();
                const bool bold = cell.value("bold").toBool(), italic = cell.value("italic").toBool(),
                           underline = cell.value("underline").toBool();
                QString text = cell.value("continuation").toBool() ? QString() : cell.value("text").toString();
                const bool simple = text.size() == 1 && text[0].unicode() < 0x80 && !cell.value("wide").toBool();
                int end = col + 1;
                if (simple) {
                    while (end < line.size()) {
                        const auto next = line[end].toMap();
                        const auto glyph = next.value("text").toString();
                        if (next.value("fg") != fg || next.value("bg") != bg || next.value("bold").toBool() != bold ||
                            next.value("italic").toBool() != italic || next.value("underline").toBool() != underline ||
                            glyph.size() != 1 || glyph[0].unicode() >= 0x80 || next.value("wide").toBool() ||
                            next.value("continuation").toBool())
                            break;
                        text += glyph;
                        ++end;
                    }
                }
                const QRect rect(1 + col * cw, 1 + row * ch, (end - col) * cw, ch);
                p->fillRect(rect, QColor(bg));
                if (!text.isEmpty() && text != QLatin1String(" ")) {
                    QFont font = bridge->font(bold, italic);
                    font.setUnderline(underline);
                    p->setFont(font);
                    p->setPen(QColor(fg));
                    if (simple) {
                        // Monospace: place each glyph on its cell boundary.
                        for (int i = 0; i < text.size(); ++i)
                            if (text[i] != ' ')
                                p->drawText(rect.x() + i * cw, rect.y() + ascent, QString(text[i]));
                    } else {
                        p->drawText(rect.x(), rect.y() + ascent, text);
                    }
                }
                col = end;
            }
        }
    }
    const auto cursor = m_cursor;
    if (cursor.size() == 2 && m_pane.value("focused").toBool() && (m_cursorShown || !hasActiveFocus())) {
        const int row = cursor[0].toInt(), col = cursor[1].toInt();
        p->setPen(QColor(bridge->frame().value("accent").toString()));
        if (editor) {
            const qreal x = 1 + cursorX(row, col);
            const qreal y = 1 + row * ch - offset;
            p->fillRect(QRectF(x, y, 2, ch), QColor(bridge->frame().value("accent").toString()));
        } else {
            p->drawRect(1 + col * cw, 1 + row * ch, cw - 1, ch - 1);
        }
    }
}
// Letters by scan code on a US layout, so Ctrl shortcuts work with
// non-Latin keyboard layouts (evdev codes + 8, as XKB/Wayland report them).
static QChar latinFromScanCode(quint32 code) {
    static const QHash<quint32, char> letters{
        {24, 'q'}, {25, 'w'}, {26, 'e'}, {27, 'r'}, {28, 't'}, {29, 'y'}, {30, 'u'}, {31, 'i'}, {32, 'o'},
        {33, 'p'}, {38, 'a'}, {39, 's'}, {40, 'd'}, {41, 'f'}, {42, 'g'}, {43, 'h'}, {44, 'j'}, {45, 'k'},
        {46, 'l'}, {52, 'z'}, {53, 'x'}, {54, 'c'}, {55, 'v'}, {56, 'b'}, {57, 'n'}, {58, 'm'}};
    const auto letter = letters.value(code, 0);
    return letter ? QChar(letter) : QChar();
}
static QString keyName(int key, const QString &text, Qt::KeyboardModifiers modifiers, quint32 scanCode) {
    switch (key) {
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
        if (key >= Qt::Key_F1 && key <= Qt::Key_F12)
            return QString("F%1").arg(key - Qt::Key_F1 + 1);
        if (key >= Qt::Key_A && key <= Qt::Key_Z)
            return QChar(key).toLower();
        // Control punctuation can have empty event text (e.g. Ctrl+,).
        if (key >= Qt::Key_Space && key <= Qt::Key_AsciiTilde)
            return QChar(key);
        if (modifiers & (Qt::ControlModifier | Qt::AltModifier))
            if (const auto latin = latinFromScanCode(scanCode); !latin.isNull())
                return latin;
        return text;
    }
}
static bool modifierOnly(int key) {
    switch (key) {
    case Qt::Key_Shift:
    case Qt::Key_Control:
    case Qt::Key_Meta:
    case Qt::Key_Alt:
    case Qt::Key_AltGr:
    case Qt::Key_Super_L:
    case Qt::Key_Super_R:
    case Qt::Key_Hyper_L:
    case Qt::Key_Hyper_R:
    case Qt::Key_CapsLock:
    case Qt::Key_NumLock:
    case Qt::Key_ScrollLock:
        return true;
    default:
        return false;
    }
}
void Bridge::key(int code, const QString &text, int modifiers, quint32 scanCode) {
    // Pressing a modifier alone is not input: forwarding it would type NUL
    // into terminals and clear their selection.
    if (modifierOnly(code))
        return;
    const auto mods = Qt::KeyboardModifiers(modifiers);
    const bool ctrl = mods.testFlag(Qt::ControlModifier), alt = mods.testFlag(Qt::AltModifier);
    send({{"action", "key"}, {"key", keyName(code, text, mods, scanCode)}, {"text", text},
          {"ctrl", ctrl}, {"alt", alt},
          {"shift", mods.testFlag(Qt::ShiftModifier) || code == Qt::Key_Backtab}});
    scheduleRefresh();
}
void CellView::keyPressEvent(QKeyEvent *e) {
    if (bridge)
        bridge->key(e->key(), e->text(), int(e->modifiers()), e->nativeScanCode());
    restartBlink();
    e->accept();
}
int CellView::rowAt(qreal y) const {
    const qreal offset = isEditor() ? m_scrollPixels : 0;
    return qMax(0, int((y - 1 + offset) / bridge->cellHeight()));
}
void CellView::click(QMouseEvent *e, const QString &kind) {
    forceActiveFocus();
    if (!m_preedit.isEmpty())
        QGuiApplication::inputMethod()->commit();
    const int row = rowAt(e->position().y());
    int column = qMax(0, int(e->position().x() - 1) / bridge->cellWidth());
    if (isEditor()) {
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
    if (isEditor() && e->button() == Qt::RightButton) {
        // Keep a selection the menu acts on; otherwise move the caret first.
        const auto editor = m_pane.value("editor").toMap();
        if (editor.value("anchor") == editor.value("cursor")) {
            QMouseEvent left(e->type(), e->position(), e->scenePosition(), e->globalPosition(),
                             Qt::LeftButton, Qt::LeftButton, e->modifiers());
            click(&left, "press");
        } else {
            forceActiveFocus();
        }
        emit contextMenuRequested(e->position().x(), e->position().y());
        e->accept();
        return;
    }
    // A press soon after a double click, near it, selects the whole line.
    if (isEditor() && e->button() == Qt::LeftButton && m_doubleClick.isValid() &&
        m_doubleClick.elapsed() < QGuiApplication::styleHints()->mouseDoubleClickInterval() &&
        (e->position() - m_doubleClickAt).manhattanLength() < 6) {
        m_doubleClick.invalidate();
        click(e, "triple");
        e->accept();
        return;
    }
    click(e, "press");
    e->accept();
}
void CellView::mouseDoubleClickEvent(QMouseEvent *e) {
    if (isEditor() && e->button() == Qt::LeftButton) {
        click(e, "double");
        m_doubleClick.start();
        m_doubleClickAt = e->position();
    } else {
        click(e, "press");
    }
    e->accept();
}
void CellView::mouseMoveEvent(QMouseEvent *e) {
    if (isEditor() && e->buttons().testFlag(Qt::RightButton)) { e->accept(); return; }
    click(e, e->buttons() == Qt::NoButton ? "move" : "drag");
    e->accept();
}
void CellView::mouseReleaseEvent(QMouseEvent *e) {
    if (isEditor() && e->button() == Qt::RightButton) { e->accept(); return; }
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
    const int ch = bridge->cellHeight(), cw = bridge->cellWidth();
    // Touchpads report pixel deltas; mouse wheels report 120 per notch.
    QPointF pixels = e->pixelDelta();
    if (pixels.isNull())
        pixels = QPointF(e->angleDelta()) / 120.0 * 3 * ch;
    if (e->modifiers().testFlag(Qt::ShiftModifier) && pixels.x() == 0)
        pixels = QPointF(pixels.y(), 0);
    if (m_pane.value("kind") == "terminal") {
        m_wheelTerminal += pixels.y();
        while (std::abs(m_wheelTerminal) >= 3 * ch) {
            const bool up = m_wheelTerminal > 0;
            m_wheelTerminal += up ? -3 * ch : 3 * ch;
            bridge->send({{"action", "pointer"},
                          {"pane", m_pane.value("id")},
                          {"row", rowAt(e->position().y())},
                          {"col", qMax(0, int(e->position().x() - 1) / cw)},
                          {"kind", up ? "wheel_up" : "wheel_down"},
                          {"shift", e->modifiers().testFlag(Qt::ShiftModifier)},
                          {"ctrl", e->modifiers().testFlag(Qt::ControlModifier)},
                          {"alt", e->modifiers().testFlag(Qt::AltModifier)}});
        }
        bridge->scheduleRefresh();
        e->accept();
        return;
    }
    if (e->modifiers().testFlag(Qt::ControlModifier)) {
        m_wheelZoom += e->angleDelta().y();
        while (std::abs(m_wheelZoom) >= 120) {
            bridge->send({{"action", "invoke_action"}, {"id", m_wheelZoom > 0 ? "zoom-in" : "zoom-out"}});
            m_wheelZoom += m_wheelZoom > 0 ? -120 : 120;
        }
        bridge->scheduleRefresh();
        e->accept();
        return;
    }
    if (pixels.x() != 0) {
        m_wheelColumns -= pixels.x();
        const int columns = int(m_wheelColumns / cw);
        if (columns != 0) {
            m_wheelColumns -= columns * cw;
            bridge->send({{"action", "scroll_columns"}, {"pane", m_pane.value("id")}, {"delta", columns}});
        }
    }
    if (pixels.y() != 0) {
        const int top = m_pane.value("editor").toMap().value("top").toInt();
        m_scrollPixels -= pixels.y();
        int lines = int(std::floor(m_scrollPixels / ch));
        if (top + lines < 0) {
            lines = -top;
            m_scrollPixels = lines * ch;
        }
        if (lines != 0) {
            m_scrollPixels -= lines * ch;
            m_requestedLines += lines;
            bridge->send({{"action", "scroll"}, {"pane", m_pane.value("id")}, {"delta", -lines}});
        }
        if (top == 0 && lines == 0 && m_scrollPixels < 0) m_scrollPixels = 0;
        emit scrollPixelsChanged();
        update();
    }
    bridge->scheduleRefresh();
    e->accept();
}
void CellView::inputMethodEvent(QInputMethodEvent *e) {
    m_preedit = e->preeditString();
    m_preeditFormats.clear();
    m_preeditCursor = -1;
    for (const auto &attribute : e->attributes()) {
        if (attribute.type == QInputMethodEvent::TextFormat) {
            const auto format = qvariant_cast<QTextFormat>(attribute.value).toCharFormat();
            if (format.isValid()) m_preeditFormats.append({attribute.start, attribute.length, format});
        } else if (attribute.type == QInputMethodEvent::Cursor) {
            m_preeditCursor = attribute.length > 0 ? attribute.start : -1;
        }
    }
    for (auto &line : m_lines) line.reset();
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
    restartBlink();
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
        auto rect = cursorRectangle();
        // Place the IME's own caret inside the composition when it has one.
        if (m_preeditCursor >= 0 && m_cursor.size() == 2 && isEditor()) {
            const int row = m_cursor[0].toInt();
            if (row < int(m_lines.size()) && m_lines[row] && m_lines[row]->lineCount() > 0) {
                const int at = textPosition(row, m_cursor[1].toInt()) + m_preeditCursor;
                rect.moveLeft(1 + m_lines[row]->lineAt(0).cursorToX(at));
            }
        }
        if (rect.isValid()) return rect;
    }
    return QQuickPaintedItem::inputMethodQuery(query);
}

// ---------------------------------------------------------------------------
Minimap::Minimap(QQuickItem *parent) : QQuickPaintedItem(parent) {
    setAcceptedMouseButtons(Qt::LeftButton);
    setOpaquePainting(true);
    connect(this, &Minimap::changed, this, [this]() { update(); });
}
void Minimap::setEditor(const QVariantMap &editor) {
    if (editor == m_editor) return;
    m_editor = editor;
    // Fetch the outline only when the document (or its content) changes.
    if (bridge && isVisible() && (editor.value("document") != m_document || editor.value("generation") != m_generation)) {
        m_document = editor.value("document");
        m_generation = editor.value("generation");
        const auto overview = bridge->overview(m_paneId);
        m_total = overview.value("total").toInt();
        m_lines.clear();
        for (const auto &entry : overview.value("lines").toList()) {
            const auto pair = entry.toList();
            if (pair.size() == 2) m_lines.append({pair[0].toInt(), pair[1].toInt()});
        }
        const int widest = overview.value("widest").toInt();
        if (widest != m_widest) {
            m_widest = widest;
            emit widestChanged();
        }
    }
    emit changed();
}
void Minimap::paint(QPainter *p) {
    if (!bridge) return;
    const QColor background(bridge->frame().value("background").toString());
    const QColor foreground(bridge->frame().value("foreground").toString());
    p->fillRect(boundingRect(), background.darker(background.lightness() > 128 ? 104 : 80));
    if (m_lines.isEmpty() || m_total <= 0) return;
    // At most 3 px per line; long documents are compressed to the height.
    const qreal lineHeight = qMin<qreal>(3.0, height() / qMax(1, m_total));
    const qreal sampleHeight = lineHeight * m_total / m_lines.size();
    const qreal scale = (width() - 6) / 120.0;
    QColor bar = foreground;
    bar.setAlphaF(0.45);
    for (int i = 0; i < m_lines.size(); ++i) {
        const auto [indent, length] = m_lines[i];
        if (length <= indent) continue;
        const qreal x = 3 + qMin(indent, 120) * scale;
        const qreal w = qMax<qreal>(1, (qMin(length, 120) - qMin(indent, 120)) * scale);
        p->fillRect(QRectF(x, i * sampleHeight, w, qMax<qreal>(1, sampleHeight * 0.7)), bar);
    }
    const int top = m_editor.value("top").toInt();
    QColor region = QColor(bridge->frame().value("accent").toString());
    region.setAlphaF(0.22);
    p->fillRect(QRectF(0, top * lineHeight, width(), qMax<qreal>(4, m_visibleRows * lineHeight)), region);
}
void Minimap::scrollTo(qreal y) {
    if (!bridge || m_total <= 0) return;
    const qreal lineHeight = qMin<qreal>(3.0, height() / qMax(1, m_total));
    const int line = qBound(0, int(y / lineHeight) - m_visibleRows / 2, m_total - 1);
    bridge->send({{"action", "scroll_to"}, {"pane", m_paneId}, {"line", line}});
    bridge->scheduleRefresh();
}
void Minimap::mousePressEvent(QMouseEvent *e) {
    scrollTo(e->position().y());
    e->accept();
}
void Minimap::mouseMoveEvent(QMouseEvent *e) {
    scrollTo(e->position().y());
    e->accept();
}

// ---------------------------------------------------------------------------
static void initializeResources() { Q_INIT_RESOURCE(resources); }
// Thread-safe request to leave the event loop (termination signals).
extern "C" void slate_qt_quit() {
    if (auto app = QCoreApplication::instance())
        QMetaObject::invokeMethod(app, "quit", Qt::QueuedConnection);
}
static void wake(void *target) {
    // Called from Rust worker threads; queue a refresh on the GUI thread.
    QMetaObject::invokeMethod(static_cast<Bridge *>(target), "scheduleRefresh", Qt::QueuedConnection);
}
extern "C" int slate_qt_run(void *context, int argc, char **argv) {
    // Qt consumes its own options (-platform, -style, -reverse…).
    static int arguments = argc;
#ifdef SLATE_SMOKE_TEST
    const bool tracing = !qEnvironmentVariableIsEmpty("SLATE_GUI_SMOKE_DIR");
    if (tracing)
        std::fprintf(stderr, "Smoke startup: constructing QApplication\n");
#endif
    QApplication app(arguments, argv);
#ifdef SLATE_SMOKE_TEST
    if (tracing)
        std::fprintf(stderr, "Smoke startup: QApplication ready\n");
#endif
    app.setApplicationName("Slate");
    app.setOrganizationName("Slate");
    app.setDesktopFileName("slate");
    initializeResources();
    app.setWindowIcon(QIcon(":/slate/slate.svg"));
    // Without a desktop platform theme (other desktops, offscreen tests),
    // use an installed freedesktop icon theme for toolbar glyphs.
    if (QIcon::themeName().isEmpty() || QIcon::themeName() == QLatin1String("hicolor")) {
        for (const auto &theme : {"breeze", "Adwaita", "Papirus", "elementary", "oxygen", "Tango"})
            for (const auto &path : QIcon::themeSearchPaths())
                if (QFileInfo::exists(path + "/" + theme + "/index.theme")) {
                    QIcon::setThemeName(theme);
                    goto themed;
                }
    themed:;
    }
    QAccessible::installFactory(accessibleFactory);
    Bridge state(context);
    bridge = &state;
    qmlRegisterType<CellView>("Slate.Native", 1, 0, "CellView");
    qmlRegisterType<Minimap>("Slate.Native", 1, 0, "Minimap");
    qmlRegisterSingletonType(QUrl("qrc:/slate/Theme.qml"), "Slate.Native", 1, 0, "Theme");
    QQmlApplicationEngine engine;
    engine.addImageProvider("icon", new IconProvider);
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
    auto window = qobject_cast<QQuickWindow *>(engine.rootObjects().first());
    state.setWindow(window);
    QTimer timer;
    QObject::connect(&timer, &QTimer::timeout, &state, &Bridge::refresh);
    timer.start(1000); // Checkpoints and the external-change watcher.
    QObject::connect(&app, &QGuiApplication::paletteChanged, &state, [&state]() {
        state.applyTheme();
        state.refresh();
    });
    // Logging out: persist recovery state (or NAME.save files) and let the
    // session end; unsaved work is restored at the next start.
    QObject::connect(&app, &QGuiApplication::commitDataRequest, &state, [&state](QSessionManager &) {
        state.send({{"action", "flush"}});
    });
    const int eventFd = slate_event_fd(context);
    QSocketNotifier *notifier =
        eventFd >= 0 ? new QSocketNotifier(eventFd, QSocketNotifier::Read, &state) : nullptr;
    if (notifier) {
        QObject::connect(notifier, &QSocketNotifier::activated, &state, [notifier, &state]() {
            notifier->setEnabled(false);
            state.scheduleRefresh();
        });
        QObject::connect(&state, &Bridge::refreshFinished, notifier, [notifier]() { notifier->setEnabled(true); });
    } else {
        // Platforms without the event descriptor wake through a callback.
        slate_set_waker(context, wake, &state);
    }
    state.refresh();

#ifdef SLATE_SMOKE_TEST
    if (!qEnvironmentVariableIsEmpty("SLATE_GUI_SMOKE_DIR"))
        startSmoke(&state, window);
#endif
#ifdef SLATE_SMOKE_TEST
    if (tracing)
        std::fprintf(stderr, "Smoke startup: entering event loop\n");
#endif
    const int result = app.exec();
    bridge = nullptr;
    return result;
}
