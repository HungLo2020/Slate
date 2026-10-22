#pragma once
#include <QAbstractListModel>
#include <QJsonArray>
#include <QJsonObject>
#include <QColor>
#include <QElapsedTimer>
#include <QFont>
#include <QGlyphRun>
#include <QRawFont>
#include <QHash>
#include <QMutex>
#include <QPointer>
#include <QQuickImageProvider>
#include <QTimer>
#include <QUrl>
#include <QObject>
#include <QQuickPaintedItem>
#include <QQuickWindow>
#include <QTextLayout>
#include <QUrl>
#include <QVariantMap>
#include <memory>
#include <vector>

class EntryModel : public QAbstractListModel {
    Q_OBJECT
  public:
    using QAbstractListModel::QAbstractListModel;
    int rowCount(const QModelIndex &parent = QModelIndex()) const override {
        return parent.isValid() ? 0 : m_rows.size();
    }
    // Rows stay compact JSON objects; delegates (visible rows only) get
    // them as maps when they ask.
    QVariant data(const QModelIndex &index, int role) const override {
        if (!index.isValid() || index.row() >= m_rows.size())
            return {};
        if (role == Qt::UserRole + 1)
            return m_rows.at(index.row()).toVariantMap();
        if (role == Qt::UserRole + 2)
            return m_rows.at(index.row()).value("group").toString();
        return {};
    }
    QHash<int, QByteArray> roleNames() const override {
        return {{Qt::UserRole + 1, "modelData"}, {Qt::UserRole + 2, "group"}};
    }
    void replace(const QJsonArray &rows);
    // Every row as maps: for tests and the occasional whole-list reader.
    QVariantList rows() const {
        QVariantList list;
        list.reserve(m_rows.size());
        for (const auto &row : m_rows) list.append(row.toVariantMap());
        return list;
    }
    QVariantMap row(int index) const {
        return index >= 0 && index < m_rows.size() ? m_rows.at(index).toVariantMap() : QVariantMap();
    }
    int size() const { return m_rows.size(); }

  private:
    QList<QJsonObject> m_rows;
};

// A terminal row is drawn as runs of equally styled cells: plain ASCII runs
// hold one character per column; any other run is one (wide or combining)
// cell drawn as a whole.
struct TerminalRun {
    int column = 0, columns = 1;
    QString text;
    QColor foreground, background;
    bool bold = false, italic = false, underline = false;
    bool perColumn = false;
    // Shaped once for the font generation it was built with: glyphs placed
    // on their cell boundaries, so a row draws in a few calls.
    mutable QGlyphRun glyphs;
    mutable int glyphsFont = -1;
};
struct TerminalGrid {
    int rows = 0, cols = 0;
    QVector<QVector<TerminalRun>> lines;
};

// Desktop theme icons for QML (`image://icon/NAME?#rrggbb`), tinted like text
// so symbolic icons remain visible on dark themes.
class IconProvider : public QQuickImageProvider {
  public:
    IconProvider() : QQuickImageProvider(QQuickImageProvider::Pixmap) {}
    QPixmap requestPixmap(const QString &id, QSize *size, const QSize &requested) override;
};

class CellView;
// Decode user images on Qt Quick's image-loading thread, with bounded output.
class PreviewProvider : public QQuickImageProvider {
  public:
    PreviewProvider() : QQuickImageProvider(QQuickImageProvider::Image, QQuickImageProvider::ForceAsynchronousImageLoading) {}
    QImage requestImage(const QString &id, QSize *size, const QSize &requested) override;
    QSize originalSize(const QString &id) const {
        QMutexLocker lock(&m_mutex);
        return m_sizes.value(id);
    }
    static QString imageId(const QString &path, qulonglong generation) {
        return QString::fromLatin1(path.toUtf8().toBase64(QByteArray::Base64UrlEncoding | QByteArray::OmitTrailingEquals)) + "?" + QString::number(generation);
    }
  private:
    mutable QMutex m_mutex;
    QHash<QString, QSize> m_sizes;
};
class QFileDialog;
class QMessageBox;
class Bridge : public QObject {
    Q_OBJECT
    // The per-update frame QML binds to, without the slower-changing groups
    // in `stable`, which notifies only when one of them changes.
    Q_PROPERTY(QVariantMap frame READ qmlFrame NOTIFY frameChanged)
    Q_PROPERTY(QVariantMap stable READ stable NOTIFY stableChanged)
    Q_PROPERTY(QVariantList paneIds READ paneIds NOTIFY structureChanged)
    Q_PROPERTY(QVariantList handleIds READ handleIds NOTIFY structureChanged)
    Q_PROPERTY(QAbstractItemModel *files READ files CONSTANT)
    Q_PROPERTY(QAbstractItemModel *git READ git CONSTANT)
    // The commit graph's rows, which follow the Git changes in the pane.
    Q_PROPERTY(QAbstractItemModel *history READ history CONSTANT)
    Q_PROPERTY(QVariantList gitEntries READ gitEntries NOTIFY gitChanged)
    // The browsed folder is below the workspace root (its listing starts with "..").
    Q_PROPERTY(bool browserNested READ browserNested NOTIFY filesChanged)
    Q_PROPERTY(int cellWidth READ cellWidth NOTIFY fontChanged)
    Q_PROPERTY(int cellHeight READ cellHeight NOTIFY fontChanged)
    Q_PROPERTY(bool pathDialogOpen READ pathDialogOpen NOTIFY pathDialogOpenChanged)
    Q_PROPERTY(bool closeDialogOpen READ closeDialogOpen NOTIFY closeDialogOpenChanged)
    Q_PROPERTY(QStringList encodings READ encodings CONSTANT)
    Q_PROPERTY(QStringList fontFamilies READ fontFamilies CONSTANT)
  public:
    Bridge(void *context, QObject *parent = nullptr);
    ~Bridge() override;
    // The whole frame, stable groups included (tests and occasional callers).
    QVariantMap frame() const {
        auto merged = m_frame;
        for (auto it = m_stable.cbegin(); it != m_stable.cend(); ++it) merged.insert(it.key(), it.value());
        return merged;
    }
    const QVariantMap &qmlFrame() const { return m_frame; }
    const QVariantMap &stable() const { return m_stable; }
    QVariantList paneIds() const { return m_paneIds; }
    QVariantList handleIds() const { return m_handleIds; }
    QAbstractItemModel *files() { return &m_files; }
    QAbstractItemModel *git() { return &m_git; }
    QAbstractItemModel *history() { return &m_history; }
    QVariantList historyEntries() const { return m_history.rows(); }
    // File and Git rows live in their models, not in the QML frame, which is
    // converted to JavaScript whenever it changes.
    QVariantList gitEntries() const { return m_git.rows(); }
    QVariantList fileEntries() const { return m_files.rows(); }
    Q_INVOKABLE QVariantMap fileEntry(int row) const {
        return m_files.row(row);
    }
    const TerminalGrid *terminal(int pane) const {
        const auto found = m_terminals.constFind(pane);
        return found == m_terminals.cend() ? nullptr : &found.value();
    }
    int cellWidth() const { return m_cellWidth; }
    int cellHeight() const { return m_cellHeight; }
    const QFont &font() const { return m_font; }
    const QFont &font(bool bold, bool italic) const { return m_variants[(bold ? 1 : 0) + (italic ? 2 : 0)]; }
    // The variant's own face, when it has a true bold/italic (synthesized
    // styles are drawn with QPainter text instead), for glyph runs.
    const QRawFont *rawFont(bool bold, bool italic) const {
        const int i = (bold ? 1 : 0) + (italic ? 2 : 0);
        return m_rawExact[i] ? &m_raw[i] : nullptr;
    }
    int fontGeneration() const { return m_fontGeneration; }
    QStringList encodings() const;
    QStringList fontFamilies() const;
    Q_INVOKABLE QVariantMap send(const QVariantMap &command) { return send(command, nullptr); }
    // As send(), moving the files/git/history lists of the reply into `lists`
    // as JSON instead of converting them with the rest.
    QVariantMap send(const QVariantMap &command, QJsonObject *lists);
    Q_INVOKABLE QVariantList commands(const QString &query);
    Q_INVOKABLE QVariantMap commandInfo(const QString &id, int pane = 0, int row = -1);
    Q_INVOKABLE QString localPath(const QUrl &url) const;
    Q_INVOKABLE QUrl fileUrl(const QString &path) const { return QUrl::fromLocalFile(path); }
    Q_INVOKABLE QString imageSource(const QString &path, qulonglong generation) const {
        return QStringLiteral("image://preview/") + PreviewProvider::imageId(path, generation);
    }
    Q_INVOKABLE QVariantMap imageSize(const QString &path, qulonglong generation) const {
        const QSize size = m_previews ? m_previews->originalSize(PreviewProvider::imageId(path, generation)) : QSize();
        return {{"width", size.width()}, {"height", size.height()}};
    }
    void setPreviewProvider(PreviewProvider *provider) { m_previews = provider; }
    Q_INVOKABLE bool hasIcon(const QString &name) const;
    // The complete native content of a pane (tests and accessibility tools).
    Q_INVOKABLE QVariantMap surface(int pane) const;
    Q_INVOKABLE QVariantMap overview(int pane);
    void applyTheme();
    Q_INVOKABLE void command(const QString &text);
    Q_INVOKABLE void viewport(int width, int height);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void scheduleRefresh();
    void tick();
    Q_INVOKABLE QVariantMap diagnostics() const;
    qulonglong inputGeneration() const { return m_inputGeneration; }
    void attachView(int id, CellView *view);
    void detachView(int id, CellView *view);
    void linkComparisonScroll(CellView *source, int lines);
    Q_INVOKABLE void paneHeader(int height);
    Q_INVOKABLE void copyClipboard();
    // Put plain text (such as a file path) on the desktop clipboard.
    Q_INVOKABLE void copyText(const QString &text);
    bool browserNested() const;
    Q_INVOKABLE void pasteClipboard();
    Q_INVOKABLE void exit();
    Q_INVOKABLE void key(int code, const QString &text, int modifiers, quint32 scanCode = 0);
    bool pathDialogOpen() const { return !m_pathDialog.isNull(); }
    Q_INVOKABLE void pickPath(const QString &kind, QObject *window);
    bool closeDialogOpen() const { return !m_closeDialog.isNull(); }
    Q_INVOKABLE void confirmCloseTab(QObject *window);
    Q_INVOKABLE void print();
    void setWindow(QQuickWindow *window) { m_window = window; }
    QQuickWindow *window() const { return m_window; }
  signals:
    void confirmationRequested(const QString &id);
    void closeDialogOpenChanged();
    void pathDialogOpenChanged();
    void frameChanged();
    void structureChanged();
    void filesChanged();
    void gitChanged();
    void historyChanged();
    void stableChanged();
    void refreshFinished();
    void fontChanged();

  private:
    EntryModel m_files, m_git, m_history;
    PreviewProvider *m_previews = nullptr;
    QPointer<QFileDialog> m_pathDialog;
    QPointer<QMessageBox> m_closeDialog;
    QPointer<QQuickWindow> m_window;
    QTimer m_refreshTimer;
    QElapsedTimer m_lastRefresh;
    bool m_refreshing = false;
    QHash<int, CellView *> m_views;
    QHash<int, QVariantMap> m_surfaces;
    QHash<int, TerminalGrid> m_terminals;
    qulonglong m_updates = 0, m_lastBytes = 0, m_clipboardReads = 0, m_catalogRequests = 0,
               m_inputContextRequests = 0;
    // Counts requests that can change state; read-only queries leave it.
    qulonglong m_inputGeneration = 0;
    void *m_context;
    QVariantMap m_frame;
    QVariantMap m_stable;
    QVariantMap m_theme;
    QHash<QString, QVariantList> m_catalogs;
    QHash<QString, QVariantList> m_rowCatalogs;
    QVariant m_catalogRevision;
    QVariantList m_paneIds, m_handleIds;
    int m_headerHeight = 43;
    int m_width = 1280, m_height = 700;
    QFont m_font;
    QFont m_variants[4];
    QRawFont m_raw[4];
    bool m_rawExact[4] = {false, false, false, false};
    int m_fontGeneration = 0;
    int m_cellWidth = 9, m_cellHeight = 18;
    QString m_fontKey;
    // The desktop clipboard, read when it changes rather than during a paste:
    // an X11 clipboard read can run a nested event loop.
    QString m_clipboard;
    void updateFont(const QVariantMap &settings);
    void performRequests(const QVariantList &requests);
};

class CellView : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(int paneId READ paneId WRITE setPaneId NOTIFY paneIdChanged)
    Q_PROPERTY(QVariantMap pane READ pane NOTIFY paneChanged)
    Q_PROPERTY(qulonglong layoutBuilds READ layoutBuilds)
    Q_PROPERTY(qreal scrollPixels READ scrollPixels NOTIFY scrollPixelsChanged)
  public:
    CellView(QQuickItem *parent = nullptr);
    ~CellView() override;
    int paneId() const { return m_paneId; }
    void setPaneId(int id);
    qulonglong layoutBuilds() const { return m_layoutBuilds; }
    qreal scrollPixels() const { return m_scrollPixels; }
    void setLinkedScroll(qreal pixels, int lines);
    void applySurface(const QVariantMap &patch);
    QVariantMap pane() const { return m_pane; }
    // The editor's input context, fetched once per state change however
    // many queries an input method makes.
    QVariantMap inputContext() const;
    void setPane(const QVariantMap &pane);
    void paint(QPainter *painter) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;
    bool isEditor() const { return m_pane.value("kind") == "editor"; }
    // Document-wide UTF-16 ranges and positions for assistive technology.
    QVariantMap accessibleContext() const;
    QVariantMap accessibleRequest(const QString &action, QVariantMap arguments = {}) const;
    QRect accessibleCharacterRect(int offset) const;
    int accessibleOffsetAt(const QPoint &point) const;
    QRectF cursorRectangle() const;
    void fontChanged();
  signals:
    void paneChanged();
    void paneIdChanged();
    void scrollPixelsChanged();
    void contextMenuRequested(qreal x, qreal y);

  protected:
    void keyPressEvent(QKeyEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void mouseReleaseEvent(QMouseEvent *event) override;
    void mouseDoubleClickEvent(QMouseEvent *event) override;
    void hoverMoveEvent(QHoverEvent *event) override;
    void hoverLeaveEvent(QHoverEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;
    void inputMethodEvent(QInputMethodEvent *event) override;
    void focusInEvent(QFocusEvent *event) override;
    void focusOutEvent(QFocusEvent *event) override;

  private:
    int m_paneId = 0;
    QVariantMap m_pane;
    mutable QVariantMap m_inputContext;
    mutable qulonglong m_inputContextAt = ~0ull;
    QVariantList m_cursor;
    QVector<QVariantMap> m_lineData;
    QVector<QVariantList> m_overlays;
    mutable QVector<QString> m_layoutPreedit;
    mutable QVector<int> m_preeditPosition;
    mutable qulonglong m_layoutBuilds = 0;
    QString m_preedit;
    QList<QTextLayout::FormatRange> m_preeditFormats;
    int m_preeditCursor = -1;
    mutable std::vector<std::unique_ptr<QTextLayout>> m_lines;
    mutable std::vector<QVector<int>> m_columns;
    // Pixel-smooth scrolling: the part of a row scrolled past `top`.
    qreal m_scrollPixels = 0;
    int m_lastTop = -1;
    int m_requestedLines = 0;
    qreal m_wheelColumns = 0, m_wheelTerminal = 0, m_wheelZoom = 0;
    QTimer m_blink;
    bool m_cursorShown = true;
    // Language-server hover text for the text the mouse rests on.
    QTimer m_hoverTimer;
    QPointF m_hoverAt;
    bool m_hoverRequested = false;
    int columnAt(int row, qreal x) const;
    QElapsedTimer m_doubleClick;
    QPointF m_doubleClickAt;
    void layoutText() const;
    int textPosition(int row, int column) const;
    qreal cursorX(int row, int column) const;
    void click(QMouseEvent *event, const QString &kind);
    int rowAt(qreal y) const;
    void restartBlink();
    void notifyAccessibleCursor();
    QVariantMap m_accessibleContext;
};

// One row of the commit graph: the lane lines passing through it (each
// [lane, height, lane, height, colour], heights 0 top, 1 middle, 2 bottom)
// and the commit's node. Curves join lanes; colours follow the core palette.
class GraphLanes : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(QVariantList segments MEMBER m_segments NOTIFY changed)
    Q_PROPERTY(int lane MEMBER m_lane NOTIFY changed)
    Q_PROPERTY(int color MEMBER m_color NOTIFY changed)
    Q_PROPERTY(bool head MEMBER m_head NOTIFY changed)
    Q_PROPERTY(bool merge MEMBER m_merge NOTIFY changed)
    Q_PROPERTY(qreal laneWidth MEMBER m_laneWidth NOTIFY changed)
    // Fills the node of a merge commit and rings the others, so nodes stay
    // distinct where lines cross them.
    Q_PROPERTY(QColor background MEMBER m_background NOTIFY changed)
  public:
    GraphLanes(QQuickItem *parent = nullptr);
    void paint(QPainter *painter) override;
    static QColor laneColor(int index);
  signals:
    void changed();

  private:
    QVariantList m_segments;
    int m_lane = 0, m_color = 0;
    bool m_head = false, m_merge = false;
    qreal m_laneWidth = 14;
    QColor m_background;
};

// The document overview beside an editor: one bar per (sampled) line, the
// visible region highlighted. Click or drag to scroll.
class Minimap : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(QColor background MEMBER m_background NOTIFY changed)
    Q_PROPERTY(QColor foreground MEMBER m_foreground NOTIFY changed)
    Q_PROPERTY(QColor accent MEMBER m_accent NOTIFY changed)
    Q_PROPERTY(int paneId MEMBER m_paneId NOTIFY changed)
    Q_PROPERTY(QVariantMap editor READ editor WRITE setEditor NOTIFY changed)
    Q_PROPERTY(int visibleRows MEMBER m_visibleRows NOTIFY changed)
    Q_PROPERTY(int widest READ widest NOTIFY widestChanged)
  public:
    Minimap(QQuickItem *parent = nullptr);
    QVariantMap editor() const { return m_editor; }
    void setEditor(const QVariantMap &editor);
    int widest() const { return m_widest; }
    void paint(QPainter *painter) override;
  signals:
    void changed();
    void widestChanged();

  protected:
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;

  private:
    int m_paneId = 0, m_visibleRows = 0, m_widest = 0, m_total = 0;
    QVariantMap m_editor;
    QColor m_background, m_foreground, m_accent;
    QVariant m_document, m_generation, m_overviewRevision;
    QVector<QPair<int, int>> m_lines;
    void scrollTo(qreal y);
};
