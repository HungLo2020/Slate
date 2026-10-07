#pragma once
#include <QAbstractListModel>
#include <QElapsedTimer>
#include <QFont>
#include <QHash>
#include <QPointer>
#include <QQuickImageProvider>
#include <QTimer>
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
    QVariant data(const QModelIndex &index, int role) const override {
        if (!index.isValid() || index.row() >= m_rows.size())
            return {};
        if (role == Qt::UserRole + 1)
            return m_rows.at(index.row());
        if (role == Qt::UserRole + 2)
            return m_rows.at(index.row()).toMap().value("group");
        return {};
    }
    QHash<int, QByteArray> roleNames() const override {
        return {{Qt::UserRole + 1, "modelData"}, {Qt::UserRole + 2, "group"}};
    }
    void replace(const QVariantList &rows);

  private:
    QVariantList m_rows;
};

// Desktop theme icons for QML (`image://icon/NAME?#rrggbb`), tinted like text
// so symbolic icons remain visible on dark themes.
class IconProvider : public QQuickImageProvider {
  public:
    IconProvider() : QQuickImageProvider(QQuickImageProvider::Pixmap) {}
    QPixmap requestPixmap(const QString &id, QSize *size, const QSize &requested) override;
};

class CellView;
class QFileDialog;
class QMessageBox;
class Bridge : public QObject {
    Q_OBJECT
    Q_PROPERTY(QVariantMap frame READ frame NOTIFY frameChanged)
    Q_PROPERTY(QVariantList paneIds READ paneIds NOTIFY structureChanged)
    Q_PROPERTY(QVariantList handleIds READ handleIds NOTIFY structureChanged)
    Q_PROPERTY(QAbstractItemModel *files READ files CONSTANT)
    Q_PROPERTY(QAbstractItemModel *git READ git CONSTANT)
    Q_PROPERTY(int cellWidth READ cellWidth NOTIFY fontChanged)
    Q_PROPERTY(int cellHeight READ cellHeight NOTIFY fontChanged)
    Q_PROPERTY(bool pathDialogOpen READ pathDialogOpen NOTIFY pathDialogOpenChanged)
    Q_PROPERTY(bool closeDialogOpen READ closeDialogOpen NOTIFY closeDialogOpenChanged)
    Q_PROPERTY(QStringList encodings READ encodings CONSTANT)
    Q_PROPERTY(QStringList fontFamilies READ fontFamilies CONSTANT)
  public:
    Bridge(void *context, QObject *parent = nullptr);
    ~Bridge() override;
    QVariantMap frame() const { return m_frame; }
    QVariantList paneIds() const { return m_paneIds; }
    QVariantList handleIds() const { return m_handleIds; }
    QAbstractItemModel *files() { return &m_files; }
    QAbstractItemModel *git() { return &m_git; }
    int cellWidth() const { return m_cellWidth; }
    int cellHeight() const { return m_cellHeight; }
    const QFont &font() const { return m_font; }
    const QFont &font(bool bold, bool italic) const { return m_variants[(bold ? 1 : 0) + (italic ? 2 : 0)]; }
    QStringList encodings() const;
    QStringList fontFamilies() const;
    Q_INVOKABLE QVariantMap send(const QVariantMap &command);
    Q_INVOKABLE QVariantList commands(const QString &query);
    Q_INVOKABLE QVariantMap commandInfo(const QString &id, int pane = 0, int row = -1);
    Q_INVOKABLE QString localPath(const QUrl &url) const;
    Q_INVOKABLE bool hasIcon(const QString &name) const;
    // The complete native content of a pane (tests and accessibility tools).
    Q_INVOKABLE QVariantMap surface(int pane) const { return m_surfaces.value(pane); }
    Q_INVOKABLE QVariantMap overview(int pane);
    void applyTheme();
    Q_INVOKABLE void command(const QString &text);
    Q_INVOKABLE void viewport(int width, int height);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void scheduleRefresh();
    Q_INVOKABLE QVariantMap diagnostics() const;
    void attachView(int id, CellView *view);
    void detachView(int id, CellView *view);
    Q_INVOKABLE void paneHeader(int height);
    Q_INVOKABLE void copyClipboard();
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
    void refreshFinished();
    void fontChanged();

  private:
    EntryModel m_files, m_git;
    QPointer<QFileDialog> m_pathDialog;
    QPointer<QMessageBox> m_closeDialog;
    QPointer<QQuickWindow> m_window;
    QTimer m_refreshTimer;
    bool m_refreshing = false;
    QHash<int, CellView *> m_views;
    QHash<int, QVariantMap> m_surfaces;
    qulonglong m_updates = 0, m_lastBytes = 0, m_clipboardReads = 0, m_catalogRequests = 0;
    void *m_context;
    QVariantMap m_frame;
    QVariantMap m_theme;
    QHash<QString, QVariantList> m_catalogs;
    QHash<QString, QVariantList> m_rowCatalogs;
    QVariant m_catalogRevision;
    QVariantList m_paneIds, m_handleIds;
    int m_headerHeight = 43;
    int m_width = 1280, m_height = 700;
    QFont m_font;
    QFont m_variants[4];
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
    void applySurface(const QVariantMap &patch);
    QVariantMap pane() const { return m_pane; }
    void setPane(const QVariantMap &pane);
    void paint(QPainter *painter) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;
    bool isEditor() const { return m_pane.value("kind") == "editor"; }
    // Accessibility: the text around the cursor and positions within it.
    QVariantMap accessibleContext() const;
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
    void wheelEvent(QWheelEvent *event) override;
    void inputMethodEvent(QInputMethodEvent *event) override;
    void focusInEvent(QFocusEvent *event) override;
    void focusOutEvent(QFocusEvent *event) override;

  private:
    int m_paneId = 0;
    QVariantMap m_pane, m_screen;
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
    QElapsedTimer m_doubleClick;
    QPointF m_doubleClickAt;
    void layoutText() const;
    int textPosition(int row, int column) const;
    qreal cursorX(int row, int column) const;
    void click(QMouseEvent *event, const QString &kind);
    int rowAt(qreal y) const;
    void restartBlink();
    void notifyAccessibleCursor();
};

// The document overview beside an editor: one bar per (sampled) line, the
// visible region highlighted. Click or drag to scroll.
class Minimap : public QQuickPaintedItem {
    Q_OBJECT
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
    QVariant m_document, m_generation;
    QVector<QPair<int, int>> m_lines;
    void scrollTo(qreal y);
};
