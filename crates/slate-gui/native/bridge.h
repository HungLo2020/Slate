#pragma once
#include <QAbstractListModel>
#include <QFont>
#include <QHash>
#include <QPointer>
#include <QTimer>
#include <QObject>
#include <QQuickPaintedItem>
#include <QTextLayout>
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
class CellView;
class QFileDialog;
class Bridge : public QObject {
    Q_OBJECT
    Q_PROPERTY(QVariantMap frame READ frame NOTIFY frameChanged)
    Q_PROPERTY(QVariantList paneIds READ paneIds NOTIFY structureChanged)
    Q_PROPERTY(QVariantList handleIds READ handleIds NOTIFY structureChanged)
    Q_PROPERTY(QAbstractItemModel *files READ files CONSTANT)
    Q_PROPERTY(QAbstractItemModel *git READ git CONSTANT)
    Q_PROPERTY(int cellWidth READ cellWidth CONSTANT)
    Q_PROPERTY(int cellHeight READ cellHeight CONSTANT)
    Q_PROPERTY(bool pathDialogOpen READ pathDialogOpen NOTIFY pathDialogOpenChanged)
  public:
    Bridge(void *context, QObject *parent = nullptr);
    ~Bridge() override;
    QVariantMap frame() const { return m_frame; }
    QVariantList paneIds() const { return m_paneIds; }
    QVariantList handleIds() const { return m_handleIds; }
    QAbstractItemModel *files() { return &m_files; }
    QAbstractItemModel *git() { return &m_git; }
    int cellWidth() const;
    int cellHeight() const;
    QFont font() const;
    Q_INVOKABLE QVariantMap send(const QVariantMap &command);
    Q_INVOKABLE QVariantList commands(const QString &query);
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
    Q_INVOKABLE void key(int code, const QString &text, int modifiers);
    bool pathDialogOpen() const { return !m_pathDialog.isNull(); }
    Q_INVOKABLE void pickPath(const QString &kind, QObject *window);
  signals:
    void pathDialogOpenChanged();
    void frameChanged();
    void structureChanged();
    void filesChanged();
    void gitChanged();
    void refreshFinished();

  private:
    EntryModel m_files, m_git;
    QPointer<QFileDialog> m_pathDialog;
    QTimer m_refreshTimer;
    bool m_refreshing = false;
    QHash<int, CellView *> m_views;
    QHash<int, QVariantMap> m_surfaces;
    qulonglong m_updates = 0, m_lastBytes = 0, m_clipboardReads = 0;
    void *m_context;
    QVariantMap m_frame;
    QVariantMap m_theme;
    QVariantList m_paneIds, m_handleIds;
    int m_headerHeight = 43;
    int m_width = 1280, m_height = 700;
};

class CellView : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(int paneId READ paneId WRITE setPaneId NOTIFY paneIdChanged)
    Q_PROPERTY(QVariantMap pane READ pane NOTIFY paneChanged)
    Q_PROPERTY(qulonglong layoutBuilds READ layoutBuilds)
  public:
    CellView(QQuickItem *parent = nullptr);
    ~CellView() override;
    int paneId() const { return m_paneId; }
    void setPaneId(int id);
    qulonglong layoutBuilds() const { return m_layoutBuilds; }
    void applySurface(const QVariantMap &patch);
    QVariantMap pane() const { return m_pane; }
    void setPane(const QVariantMap &pane);
    void paint(QPainter *painter) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;
  signals:
    void paneChanged();
    void paneIdChanged();

  protected:
    void keyPressEvent(QKeyEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void mouseReleaseEvent(QMouseEvent *event) override;
    void hoverMoveEvent(QHoverEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;
    void inputMethodEvent(QInputMethodEvent *event) override;

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
    mutable std::vector<std::unique_ptr<QTextLayout>> m_lines;
    mutable std::vector<QVector<int>> m_columns;
    void layoutText() const;
    int textPosition(int row, int column) const;
    qreal cursorX(int row, int column) const;
    void click(QMouseEvent *event, const QString &kind);
};
