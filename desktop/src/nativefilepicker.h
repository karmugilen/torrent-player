#pragma once

#include "filepicker.h"

#include <QPointer>

class QFileDialog;

// The platform's own file dialog through QFileDialog (Windows and macOS; Linux uses
// the portal). Non-blocking: the dialog opens and picked() follows when it closes.
class NativeFilePicker : public FilePicker {
    Q_OBJECT

public:
    using FilePicker::FilePicker;
    ~NativeFilePicker() override;
    void open(const QString &kind) override;

    // Name filters for QFileDialog, empty for "directory". Pure, for tests.
    static QStringList nameFilters(const QString &kind);

private:
    QPointer<QFileDialog> m_dialog;
};
