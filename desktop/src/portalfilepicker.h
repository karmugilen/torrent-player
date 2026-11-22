#pragma once

#include "filepicker.h"

#include <QVariantMap>

// Opens files through org.freedesktop.portal.FileChooser, so the picker is
// the desktop's own and works the same under Flatpak.
class PortalFilePicker : public FilePicker {
    Q_OBJECT

public:
    using FilePicker::FilePicker;
    void open(const QString &kind) override;

private slots:
    void handleResponse(uint response, const QVariantMap &results);

private:
    QString m_kind;
    QString m_requestPath;
};
