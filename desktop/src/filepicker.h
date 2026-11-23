#pragma once

#include <QObject>

// Asks the user for a file. Linux uses the desktop portal (PortalFilePicker), Windows
// and macOS the native dialog (NativeFilePicker); tests can put a
// fake behind the same interface.
class FilePicker : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool busy READ busy NOTIFY busyChanged)

public:
    using QObject::QObject;
    bool busy() const { return m_busy; }

    // kind: "torrent", "subtitle", or "directory".
    Q_INVOKABLE virtual void open(const QString &kind) = 0;

signals:
    // tag repeats the kind passed to open().
    void picked(const QString &kind, const QString &path);
    void failed(const QString &message);
    void busyChanged();

protected:
    void setBusy(bool busy) {
        if (busy != m_busy) {
            m_busy = busy;
            emit busyChanged();
        }
    }

private:
    bool m_busy = false;
};
