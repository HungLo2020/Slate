#pragma once
#include <QFont>
#include <QObject>
#include <QQuickPaintedItem>
#include <QVariantMap>

class Bridge : public QObject {
    Q_OBJECT
    Q_PROPERTY(QVariantMap frame READ frame NOTIFY frameChanged)
    Q_PROPERTY(QVariantList paneIds READ paneIds NOTIFY structureChanged)
    Q_PROPERTY(QVariantList handleIds READ handleIds NOTIFY structureChanged)
    Q_PROPERTY(QVariantList files READ files NOTIFY filesChanged)
    Q_PROPERTY(QVariantList git READ git NOTIFY gitChanged)
    Q_PROPERTY(int cellWidth READ cellWidth CONSTANT)
    Q_PROPERTY(int cellHeight READ cellHeight CONSTANT)
  public:
    Bridge(void *context, QObject *parent = nullptr);
    QVariantMap frame() const { return m_frame; }
    QVariantList paneIds() const { return m_paneIds; }
    QVariantList handleIds() const { return m_handleIds; }
    QVariantList files() const { return m_frame.value("files").toList(); }
    QVariantList git() const { return m_frame.value("git").toList(); }
    int cellWidth() const;
    int cellHeight() const;
    QFont font() const;
    Q_INVOKABLE QVariantMap send(const QVariantMap &command);
    Q_INVOKABLE void command(const QString &text);
    Q_INVOKABLE void viewport(int width, int height);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void copyClipboard();
    Q_INVOKABLE void pasteClipboard();
    Q_INVOKABLE void exit();
    Q_INVOKABLE void key(int code, const QString &text, int modifiers);
  signals:
    void frameChanged();
    void structureChanged();
    void filesChanged();
    void gitChanged();

  private:
    void *m_context;
    QVariantMap m_frame;
    QVariantList m_paneIds, m_handleIds;
    int m_width = 1280, m_height = 700;
};

class CellView : public QQuickPaintedItem {
    Q_OBJECT
    Q_PROPERTY(QVariantMap pane READ pane WRITE setPane NOTIFY paneChanged)
  public:
    CellView(QQuickItem *parent = nullptr);
    QVariantMap pane() const { return m_pane; }
    void setPane(const QVariantMap &pane);
    void paint(QPainter *painter) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;
  signals:
    void paneChanged();

  protected:
    void keyPressEvent(QKeyEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void mouseReleaseEvent(QMouseEvent *event) override;
    void hoverMoveEvent(QHoverEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;
    void inputMethodEvent(QInputMethodEvent *event) override;

  private:
    QVariantMap m_pane;
    void click(QMouseEvent *event, const QString &kind);
};
