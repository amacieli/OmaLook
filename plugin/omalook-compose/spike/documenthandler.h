#pragma once
#include <QObject>
#include <QQuickTextDocument>
#include <QtQml/qqmlregistration.h>

class DocumentHandler : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QQuickTextDocument* document READ document WRITE setDocument NOTIFY documentChanged)
    Q_PROPERTY(int selectionStart MEMBER m_start NOTIFY selectionChanged)
    Q_PROPERTY(int selectionEnd MEMBER m_end NOTIFY selectionChanged)
public:
    QQuickTextDocument* document() const { return m_doc; }
    void setDocument(QQuickTextDocument* d) { m_doc = d; emit documentChanged(); }
    Q_INVOKABLE void toggleBold();
    Q_INVOKABLE QString html() const;
    Q_INVOKABLE QString qtVersion() const;
signals:
    void documentChanged();
    void selectionChanged();
private:
    QQuickTextDocument* m_doc = nullptr;
    int m_start = 0, m_end = 0;
};
