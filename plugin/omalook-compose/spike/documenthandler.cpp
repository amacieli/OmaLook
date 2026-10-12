#include "documenthandler.h"
#include <QTextCursor>
#include <QTextDocument>
#include <QTextCharFormat>

void DocumentHandler::toggleBold() {
    if (!m_doc || !m_doc->textDocument()) return;
    QTextCursor c(m_doc->textDocument());
    c.setPosition(m_start); c.setPosition(m_end, QTextCursor::KeepAnchor);
    QTextCharFormat f; f.setFontWeight(QFont::Bold);
    c.mergeCharFormat(f);
}
QString DocumentHandler::html() const { return m_doc ? m_doc->textDocument()->toHtml() : QString(); }
QString DocumentHandler::qtVersion() const { return QString::fromLatin1(qVersion()); }
