#pragma once

// Picture-in-picture for the built-in player, like Android's PiP: the window
// shrinks to a small video-only view that stays on top in a corner of the
// screen, and goes back to its old size and place when PiP ends.
//
// On Hyprland a client can't move, resize or pin itself, so the window is
// floated, pinned and placed through hyprctl (0.55+ Lua dispatchers) and put
// back the same way. Elsewhere Qt::WindowStaysOnTopHint keeps it on top.

#include <QObject>
#include <QPointer>
#include <QRect>
#include <QSize>
#include <QStringList>
#include <QWindow>

class PipWindow : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool active READ active NOTIFY activeChanged)

public:
    // What hyprctl -j clients says about one window.
    struct HyprClient {
        QString address;
        bool floating = false;
        bool pinned = false;
        int monitor = -1;
        int fullscreen = 0;
        QRect geometry;
        bool isValid() const { return !address.isEmpty(); }
    };

    static constexpr int kMargin = 24;
    static constexpr int kWidth = 480;
    static constexpr int kHeight = 270;

    explicit PipWindow(QObject *parent = nullptr);
    ~PipWindow() override;

    bool active() const { return m_active; }

    // aspect: the video's width / height; 0 or less means 16:9.
    Q_INVOKABLE void enter(QWindow *window, double aspect = 16.0 / 9.0);
    Q_INVOKABLE void leave();

    // ---- pure helpers (unit tested) ----

    // The PiP size: the video's shape inside a box of about 480x270.
    static QSize pipSize(double aspect, QSize box = QSize(kWidth, kHeight));
    // Top-left of a window of `size` in the bottom-right corner of `area`.
    static QPoint cornerPosition(const QRect &area, const QSize &size, int margin = kMargin);
    // The mapped window of process `pid` in hyprctl -j clients output.
    static HyprClient findClient(const QByteArray &clientsJson, qint64 pid);
    // A monitor's usable area in logical pixels (reserved bars taken off).
    static QRect monitorArea(const QByteArray &monitorsJson, int monitorId);
    // hl.dsp.* expressions (for hl.dispatch) to enter PiP on Hyprland 0.55+.
    static QStringList enterCommands(const HyprClient &client, const QSize &size, const QPoint &position);
    // ... and to undo it: `before` is the window before PiP, `now` as it is now.
    static QStringList leaveCommands(const HyprClient &before, const HyprClient &now);
    // hyprctl eval script: successive hl.dispatch(...) calls.
    static QString batch(const QStringList &commands);

signals:
    void activeChanged();

private:
    static bool onHyprland();
    static QByteArray hyprctl(const QStringList &arguments);

    bool m_active = false;
    QPointer<QWindow> m_window;
    // Saved on enter, put back on leave.
    QRect m_geometry;
    Qt::WindowStates m_states;
    bool m_stayedOnTop = false;
    bool m_hyprland = false;
    HyprClient m_before;
};
