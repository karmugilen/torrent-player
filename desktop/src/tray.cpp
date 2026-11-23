#include "tray.h"

#include "format.h"
#include "library.h"

#include <QAction>
#include <QApplication>
#include <QIcon>
#include <QMenu>
#include <QQuickWindow>

Tray::Tray(Library *library, QQuickWindow *window, QObject *parent)
    : QObject(parent), m_library(library), m_window(window) {
    m_icon.setIcon(QIcon::fromTheme(QStringLiteral("torrentplayer"), qApp->windowIcon()));
    m_icon.setToolTip(tr("Torrent Player"));
    rebuildMenu();
    connect(&m_icon, &QSystemTrayIcon::activated, this, &Tray::onActivated);
    if (m_library) {
        connect(m_library, &Library::revisionChanged, this, &Tray::updateTooltip);
        connect(m_library, &Library::countChanged, this, &Tray::updateTooltip);
    }
    updateTooltip();
    if (available())
        m_icon.show();
}

void Tray::setBackgroundHintShown(bool shown) {
    if (m_backgroundHintShown == shown)
        return;
    m_backgroundHintShown = shown;
    emit backgroundHintShownChanged();
}

void Tray::setWindow(QQuickWindow *window) { m_window = window; }

void Tray::showWindow() {
    if (!m_window)
        return;
    m_window->show();
    m_window->raise();
    m_window->requestActivate();
}

void Tray::hideToTray() {
    if (!m_window)
        return;
    const bool first = !m_backgroundHintShown;
    m_window->hide();
    if (first) {
        setBackgroundHintShown(true);
        emit firstHideToTray();
        if (available()) {
            m_icon.showMessage(tr("Torrent Player"),
                               tr("Torrent Player keeps downloading in the background"),
                               QSystemTrayIcon::Information, 4000);
        }
    }
}

void Tray::updateTooltip() {
    if (!m_library) {
        m_icon.setToolTip(tr("Torrent Player"));
        return;
    }
    const int active = m_library->activeCount();
    const QString down = Format::speed(m_library->downloadSpeed());
    const QString up = Format::speed(m_library->uploadSpeed());
    if (active <= 0) {
        m_icon.setToolTip(tr("Torrent Player"));
        return;
    }
    m_icon.setToolTip(tr("Torrent Player — %n download(s)\n↓ %1  ↑ %2", "", active).arg(down, up));
    rebuildMenu();
}

void Tray::onActivated(QSystemTrayIcon::ActivationReason reason) {
    if (reason == QSystemTrayIcon::Trigger || reason == QSystemTrayIcon::DoubleClick) {
        if (m_window && m_window->isVisible())
            m_window->hide();
        else
            showWindow();
    }
}

void Tray::rebuildMenu() {
    if (!m_menu) {
        m_menu = new QMenu();
        m_icon.setContextMenu(m_menu);
    }
    m_menu->clear();
    QAction *show = m_menu->addAction(tr("Show"));
    connect(show, &QAction::triggered, this, &Tray::showWindow);

    const bool anyActive = m_library && m_library->activeCount() > 0;
    // Prefer Resume when everything looks paused (activeCount is transferring).
    bool anyUnpaused = false;
    if (m_library) {
        for (const QString &key : m_library->libraryKeys()) {
            if (const auto *e = m_library->entry(key); e && !e->paused) {
                anyUnpaused = true;
                break;
            }
        }
    }
    if (anyUnpaused) {
        QAction *pause = m_menu->addAction(tr("Pause all"));
        connect(pause, &QAction::triggered, m_library, &Library::pauseAll);
    } else {
        QAction *resume = m_menu->addAction(tr("Resume all"));
        connect(resume, &QAction::triggered, m_library, &Library::resumeAll);
        resume->setEnabled(m_library && m_library->count() > 0);
    }
    Q_UNUSED(anyActive);
    m_menu->addSeparator();
    QAction *quit = m_menu->addAction(tr("Quit"));
    connect(quit, &QAction::triggered, this, &Tray::quitRequested);
}
