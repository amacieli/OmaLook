#pragma once
#include <QColor>
#include <QObject>
#include <QQuickTextDocument>
#include <QTextCursor>
#include <QtQml/qqmlregistration.h>

// Formatting engine for the compose editor. A QML TextEdit cannot change the format of
// its selection by itself; this attaches to the TextEdit's document and does it through
// QTextCursor. Only editing logic lives here — every visual is QML.
class DocumentHandler : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QQuickTextDocument *document READ document WRITE setDocument NOTIFY documentChanged)
    Q_PROPERTY(int cursorPosition READ cursorPosition WRITE setCursorPosition NOTIFY cursorChanged)
    Q_PROPERTY(int selectionStart READ selectionStart WRITE setSelectionStart NOTIFY cursorChanged)
    Q_PROPERTY(int selectionEnd READ selectionEnd WRITE setSelectionEnd NOTIFY cursorChanged)
    // Format at the cursor / of the selection, for the toolbar's pressed states.
    Q_PROPERTY(bool bold READ bold NOTIFY formatChanged)
    Q_PROPERTY(bool italic READ italic NOTIFY formatChanged)
    Q_PROPERTY(bool underline READ underline NOTIFY formatChanged)
    Q_PROPERTY(bool strike READ strike NOTIFY formatChanged)
    Q_PROPERTY(bool bulletList READ bulletList NOTIFY formatChanged)
    Q_PROPERTY(bool numberedList READ numberedList NOTIFY formatChanged)
    Q_PROPERTY(QString fontFamily READ fontFamily NOTIFY formatChanged)
    Q_PROPERTY(int fontSize READ fontSize NOTIFY formatChanged)
    Q_PROPERTY(QColor textColor READ textColor NOTIFY formatChanged)
    Q_PROPERTY(int alignment READ alignment NOTIFY formatChanged)
    // True once any formatting beyond plain paragraphs exists (decides text/plain vs HTML).
    Q_PROPERTY(bool hasFormatting READ hasFormatting NOTIFY contentChanged)
    // True when the document carries font / size / colour / highlight / alignment, which
    // system-font mode cannot send.
    Q_PROPERTY(bool hasRichAttributes READ hasRichAttributes NOTIFY contentChanged)

public:
    QQuickTextDocument *document() const { return m_doc; }
    void setDocument(QQuickTextDocument *d);
    int cursorPosition() const { return m_cursor; }
    void setCursorPosition(int p);
    int selectionStart() const { return m_start; }
    void setSelectionStart(int p);
    int selectionEnd() const { return m_end; }
    void setSelectionEnd(int p);

    bool bold() const;
    bool italic() const;
    bool underline() const;
    bool strike() const;
    bool bulletList() const;
    bool numberedList() const;
    QString fontFamily() const;
    int fontSize() const;
    QColor textColor() const;
    int alignment() const;
    bool hasFormatting() const;
    bool hasRichAttributes() const;

    Q_INVOKABLE void toggleBold();
    Q_INVOKABLE void toggleItalic();
    Q_INVOKABLE void toggleUnderline();
    Q_INVOKABLE void toggleStrike();
    Q_INVOKABLE void toggleBulletList();
    Q_INVOKABLE void toggleNumberedList();
    Q_INVOKABLE void setFontFamily(const QString &family);
    Q_INVOKABLE void setFontSize(int pixels);
    Q_INVOKABLE void setTextColor(const QColor &c);
    Q_INVOKABLE void setHighlight(const QColor &c);
    Q_INVOKABLE void setAlignment(int qtAlignment);
    Q_INVOKABLE void insertLink(const QString &url, const QString &text);
    Q_INVOKABLE void toggleQuote();
    // Remove character formatting (HTML mode) from the selection.
    Q_INVOKABLE void clearFormatting();
    // Strip every attribute system-font mode cannot send (font, size, colour, highlight,
    // alignment) from the whole document. Called when switching HTML -> system.
    Q_INVOKABLE void flattenToSystem();
    // Replace the document with HTML / plain text.
    Q_INVOKABLE void setHtml(const QString &html);
    Q_INVOKABLE void setPlainText(const QString &text);
    // Raw QTextDocument output. The Rust side's compose_html::clean() tidies it for sending.
    Q_INVOKABLE QString html() const;
    Q_INVOKABLE QString plainText() const;
    Q_INVOKABLE QString qtVersion() const;

signals:
    void documentChanged();
    void cursorChanged();
    void formatChanged();
    void contentChanged();

private:
    QTextDocument *doc() const;
    QTextCursor cursor() const;                 // selection if any, else the word under the caret
    QTextCursor rawCursor() const;              // exactly the selection / caret
    void mergeFormat(const QTextCharFormat &f);
    void toggleList(QTextListFormat::Style style);
    bool listIs(QTextListFormat::Style style) const;
    QTextCharFormat charFormat() const;

    QQuickTextDocument *m_doc = nullptr;
    int m_cursor = 0, m_start = 0, m_end = 0;
};
