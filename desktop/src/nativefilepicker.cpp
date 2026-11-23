#include "nativefilepicker.h"

#include <QFileDialog>
#include <QStandardPaths>

NativeFilePicker::~NativeFilePicker() {
    if (m_dialog)
        delete m_dialog;
}

QStringList NativeFilePicker::nameFilters(const QString &kind) {
    if (kind == QLatin1String("directory"))
        return {};
    if (kind == QLatin1String("subtitle"))
        return {tr("Subtitles (*.srt *.vtt *.SRT *.VTT)")};
    return {tr("Torrent files (*.torrent)")};
}

void NativeFilePicker::open(const QString &kind) {
    if (busy())
        return;
    const bool directory = kind == QLatin1String("directory");
    // No parent widget: the window is Qt Quick. The dialog is application-modal instead.
    auto *dialog = new QFileDialog;
    m_dialog = dialog;
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->setWindowModality(Qt::ApplicationModal);
    dialog->setAcceptMode(QFileDialog::AcceptOpen);
    if (directory) {
        dialog->setWindowTitle(tr("Choose save folder"));
        dialog->setFileMode(QFileDialog::Directory);
        dialog->setOption(QFileDialog::ShowDirsOnly, true);
    } else {
        dialog->setWindowTitle(kind == QLatin1String("subtitle") ? tr("Open subtitles") : tr("Open torrent file"));
        dialog->setFileMode(QFileDialog::ExistingFile);
        dialog->setNameFilters(nameFilters(kind));
    }
    const QString start = QStandardPaths::writableLocation(directory ? QStandardPaths::HomeLocation
                                                                     : QStandardPaths::DownloadLocation);
    if (!start.isEmpty())
        dialog->setDirectory(start);

    connect(dialog, &QFileDialog::finished, this, [this, dialog, kind](int result) {
        setBusy(false);
        if (result != QDialog::Accepted)
            return;
        const QStringList files = dialog->selectedFiles();
        if (!files.isEmpty() && !files.first().isEmpty())
            emit picked(kind, files.first());
    });
    setBusy(true);
    dialog->open();
}
