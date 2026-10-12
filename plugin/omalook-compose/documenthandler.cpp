#include "documenthandler.h"
#include <QFont>
#include <QTextBlock>
#include <QTextCharFormat>
#include <QTextDocument>
#include <QTextList>

QTextDocument *DocumentHandler::doc() const { return m_doc ? m_doc->textDocument() : nullptr; }

void DocumentHandler::setDocument(QQuickTextDocument *d) {
    if (m_doc == d) return;
    if (doc()) disconnect(doc(), nullptr, this, nullptr);
    m_doc = d;
    if (doc()) {
        connect(doc(), &QTextDocument::contentsChanged, this, &DocumentHandler::contentChanged);
        connect(doc(), &QTextDocument::contentsChanged, this, &DocumentHandler::formatChanged);
    }
    emit documentChanged();
    emit formatChanged();
}

void DocumentHandler::setCursorPosition(int p) { if (m_cursor != p) { m_cursor = p; emit cursorChanged(); emit formatChanged(); } }
void DocumentHandler::setSelectionStart(int p)  { if (m_start != p)  { m_start = p;  emit cursorChanged(); emit formatChanged(); } }
void DocumentHandler::setSelectionEnd(int p)    { if (m_end != p)    { m_end = p;    emit cursorChanged(); emit formatChanged(); } }

QTextCursor DocumentHandler::rawCursor() const {
    QTextCursor c(doc());
    if (!doc()) return c;
    const int len = qMax(0, doc()->characterCount() - 1);
    if (m_start != m_end) {
        c.setPosition(qBound(0, qMin(m_start, m_end), len));
        c.setPosition(qBound(0, qMax(m_start, m_end), len), QTextCursor::KeepAnchor);
    } else {
        c.setPosition(qBound(0, m_cursor, len));
    }
    return c;
}

QTextCursor DocumentHandler::cursor() const {
    QTextCursor c = rawCursor();
    if (!c.hasSelection()) c.select(QTextCursor::WordUnderCursor);
    return c;
}

void DocumentHandler::mergeFormat(const QTextCharFormat &f) {
    if (!doc()) return;
    QTextCursor c = rawCursor();
    // No selection: format the word under the caret (the editor can't carry a pending format).
    if (!c.hasSelection()) c = cursor();
    c.mergeCharFormat(f);
    emit formatChanged();
}

QTextCharFormat DocumentHandler::charFormat() const {
    if (!doc()) return QTextCharFormat();
    QTextCursor c = rawCursor();
    return c.charFormat();
}

bool DocumentHandler::bold() const { return charFormat().fontWeight() > QFont::Medium; }
bool DocumentHandler::italic() const { return charFormat().fontItalic(); }
bool DocumentHandler::underline() const { return charFormat().fontUnderline(); }
bool DocumentHandler::strike() const { return charFormat().fontStrikeOut(); }
QString DocumentHandler::fontFamily() const { return charFormat().fontFamilies().toStringList().value(0, charFormat().font().family()); }
int DocumentHandler::fontSize() const { const auto f = charFormat(); return f.fontPointSize() > 0 ? int(f.fontPointSize()) : f.font().pixelSize(); }
QColor DocumentHandler::textColor() const { return charFormat().foreground().color(); }
int DocumentHandler::alignment() const { return doc() ? int(rawCursor().blockFormat().alignment()) : int(Qt::AlignLeft); }

void DocumentHandler::toggleBold()      { QTextCharFormat f; f.setFontWeight(bold() ? QFont::Normal : QFont::Bold); mergeFormat(f); }
void DocumentHandler::toggleItalic()    { QTextCharFormat f; f.setFontItalic(!italic()); mergeFormat(f); }
void DocumentHandler::toggleUnderline() { QTextCharFormat f; f.setFontUnderline(!underline()); mergeFormat(f); }
void DocumentHandler::toggleStrike()    { QTextCharFormat f; f.setFontStrikeOut(!strike()); mergeFormat(f); }

void DocumentHandler::setFontFamily(const QString &family) { QTextCharFormat f; f.setFontFamilies({family}); mergeFormat(f); }
void DocumentHandler::setFontSize(int px) { if (px <= 0) return; QTextCharFormat f; f.setProperty(QTextFormat::FontPixelSize, px); mergeFormat(f); }
void DocumentHandler::setTextColor(const QColor &c) { QTextCharFormat f; f.setForeground(c); mergeFormat(f); }
void DocumentHandler::setHighlight(const QColor &c) { QTextCharFormat f; f.setBackground(c); mergeFormat(f); }

void DocumentHandler::setAlignment(int a) {
    if (!doc()) return;
    QTextBlockFormat bf; bf.setAlignment(Qt::Alignment(a));
    rawCursor().mergeBlockFormat(bf);
    emit formatChanged();
}

bool DocumentHandler::listIs(QTextListFormat::Style style) const {
    if (!doc()) return false;
    QTextList *l = rawCursor().currentList();
    return l && l->format().style() == style;
}
bool DocumentHandler::bulletList() const { return listIs(QTextListFormat::ListDisc); }
bool DocumentHandler::numberedList() const { return listIs(QTextListFormat::ListDecimal); }

void DocumentHandler::toggleList(QTextListFormat::Style style) {
    if (!doc()) return;
    QTextCursor c = rawCursor();
    c.beginEditBlock();
    if (listIs(style)) {
        QTextBlockFormat bf = c.blockFormat();
        bf.setObjectIndex(-1); bf.setIndent(0);
        c.setBlockFormat(bf);
    } else {
        QTextListFormat lf; lf.setStyle(style); lf.setIndent(1);
        c.createList(lf);
    }
    c.endEditBlock();
    emit formatChanged();
}
void DocumentHandler::toggleBulletList()   { toggleList(QTextListFormat::ListDisc); }
void DocumentHandler::toggleNumberedList() { toggleList(QTextListFormat::ListDecimal); }

void DocumentHandler::insertLink(const QString &url, const QString &text) {
    if (!doc() || url.isEmpty()) return;
    QTextCursor c = rawCursor();
    QTextCharFormat f;
    f.setAnchor(true); f.setAnchorHref(url); f.setFontUnderline(true);
    if (c.hasSelection() && text.isEmpty()) c.mergeCharFormat(f);
    else c.insertText(text.isEmpty() ? url : text, f);
    emit formatChanged();
}

void DocumentHandler::toggleQuote() {
    if (!doc()) return;
    QTextCursor c = rawCursor();
    QTextBlockFormat bf = c.blockFormat();
    const bool on = bf.leftMargin() > 0;
    bf.setLeftMargin(on ? 0 : 16);
    bf.setProperty(QTextFormat::BlockQuoteLevel, on ? 0 : 1);
    c.setBlockFormat(bf);
    emit formatChanged();
}

void DocumentHandler::clearFormatting() {
    if (!doc()) return;
    QTextCursor c = rawCursor();
    if (!c.hasSelection()) c = cursor();
    c.setCharFormat(QTextCharFormat());
    emit formatChanged();
}

void DocumentHandler::flattenToSystem() {
    if (!doc()) return;
    QTextCursor c(doc());
    c.beginEditBlock();
    for (QTextBlock b = doc()->begin(); b.isValid(); b = b.next()) {
        QTextCursor bc(b);
        QTextBlockFormat bf = bc.blockFormat();
        bf.setAlignment(Qt::AlignLeft);
        bc.setBlockFormat(bf);
        for (auto it = b.begin(); !it.atEnd(); ++it) {
            const QTextFragment fr = it.fragment();
            QTextCharFormat f = fr.charFormat();
            f.clearProperty(QTextFormat::FontFamilies);
            f.clearProperty(QTextFormat::FontFamily);
            f.clearProperty(QTextFormat::FontPointSize);
            f.clearProperty(QTextFormat::FontPixelSize);
            f.clearForeground();
            f.clearBackground();
            QTextCursor fc(doc());
            fc.setPosition(fr.position());
            fc.setPosition(fr.position() + fr.length(), QTextCursor::KeepAnchor);
            fc.setCharFormat(f);
        }
    }
    c.endEditBlock();
    emit formatChanged();
}

bool DocumentHandler::hasRichAttributes() const {
    if (!doc()) return false;
    for (QTextBlock b = doc()->begin(); b.isValid(); b = b.next()) {
        if (b.blockFormat().alignment() & (Qt::AlignHCenter | Qt::AlignRight | Qt::AlignJustify)) return true;
        for (auto it = b.begin(); !it.atEnd(); ++it) {
            const QTextCharFormat f = it.fragment().charFormat();
            if (f.hasProperty(QTextFormat::FontFamilies) || f.hasProperty(QTextFormat::FontFamily)
                || f.hasProperty(QTextFormat::FontPointSize) || f.hasProperty(QTextFormat::FontPixelSize)
                || f.hasProperty(QTextFormat::ForegroundBrush) || f.hasProperty(QTextFormat::BackgroundBrush))
                return true;
        }
    }
    return false;
}

bool DocumentHandler::hasFormatting() const {
    if (!doc()) return false;
    for (QTextBlock b = doc()->begin(); b.isValid(); b = b.next()) {
        if (b.textList() || b.blockFormat().leftMargin() > 0) return true;
        for (auto it = b.begin(); !it.atEnd(); ++it) {
            const QTextCharFormat f = it.fragment().charFormat();
            if (f.fontWeight() > QFont::Medium || f.fontItalic() || f.fontUnderline() || f.fontStrikeOut() || f.isAnchor())
                return true;
        }
    }
    return false;
}

void DocumentHandler::setHtml(const QString &h) { if (doc()) doc()->setHtml(h); }
void DocumentHandler::setPlainText(const QString &t) { if (doc()) doc()->setPlainText(t); }
QString DocumentHandler::html() const { return doc() ? doc()->toHtml() : QString(); }
QString DocumentHandler::plainText() const { return doc() ? doc()->toPlainText() : QString(); }
QString DocumentHandler::qtVersion() const { return QString::fromLatin1(qVersion()); }
