#pragma once

#include <QObject>
#include <QSystemTrayIcon>

class Library;
class QMenu;
class QQuickWindow;

// StatusNotifierItem / system tray: show, pause/resume all, quit. Closing the
// main window hides to the tray; Quit asks when downloads are active.
class Tray : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool available READ available CONSTANT)
    Q_PROPERTY(bool backgroundHintShown READ backgroundHintShown WRITE setBackgroundHintShown NOTIFY backgroundHintShownChanged)

public:
    Tray(Library *library, QQuickWindow *window = nullptr, QObject *parent = nullptr);

    bool available() const { return m_icon.isSystemTrayAvailable(); }
    bool backgroundHintShown() const { return m_backgroundHintShown; }
    void setBackgroundHintShown(bool shown);
    void setWindow(QQuickWindow *window);

    Q_INVOKABLE void showWindow();
    Q_INVOKABLE void hideToTray();
    Q_INVOKABLE void updateTooltip();

signals:
    void quitRequested();
    void backgroundHintShownChanged();
    void firstHideToTray();

private slots:
    void onActivated(QSystemTrayIcon::ActivationReason reason);

private:
    void rebuildMenu();

    Library *m_library = nullptr;
    QQuickWindow *m_window = nullptr;
    QSystemTrayIcon m_icon;
    QMenu *m_menu = nullptr;
    bool m_backgroundHintShown = false;
};
