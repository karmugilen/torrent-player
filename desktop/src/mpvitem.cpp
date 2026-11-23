#include "mpvitem.h"

#include "platform.h"

#include <QGuiApplication>
#include <QMetaObject>
#include <QOpenGLContext>
#include <QOpenGLFramebufferObject>
#include <QQuickWindow>
#include <QTimer>
#include <QtGui/qguiapplication_platform.h>
#include <clocale>
#include <mutex>
#include <mpv/client.h>
#include <mpv/render_gl.h>

// The item may go before its renderer (which lives on the render thread), so mpv's
// callbacks reach the item through this, cleared when the item is destroyed.
struct MpvShared {
    std::mutex lock;
    MpvPlayer *item = nullptr;

    void post(const char *method) {
        std::lock_guard<std::mutex> guard(lock);
        if (item)
            QMetaObject::invokeMethod(item, method, Qt::QueuedConnection);
    }
};

namespace {

enum Reply : quint64 { ScreenshotBase = 1 << 20 };

void wakeup(void *ctx) { static_cast<MpvShared *>(ctx)->post("onMpvEvents"); }
void renderUpdate(void *ctx) { static_cast<MpvShared *>(ctx)->post("update"); }

void *procAddress(void *, const char *name) {
    QOpenGLContext *gl = QOpenGLContext::currentContext();
    return gl ? reinterpret_cast<void *>(gl->getProcAddress(QByteArray(name))) : nullptr;
}

QVariant nodeToVariant(const mpv_node &n) {
    switch (n.format) {
    case MPV_FORMAT_STRING: return QString::fromUtf8(n.u.string);
    case MPV_FORMAT_FLAG: return bool(n.u.flag);
    case MPV_FORMAT_INT64: return qlonglong(n.u.int64);
    case MPV_FORMAT_DOUBLE: return n.u.double_;
    case MPV_FORMAT_NODE_MAP: {
        QVariantMap map;
        for (int i = 0; i < n.u.list->num; ++i)
            map.insert(QString::fromUtf8(n.u.list->keys[i]), nodeToVariant(n.u.list->values[i]));
        return map;
    }
    case MPV_FORMAT_NODE_ARRAY: {
        QVariantList list;
        for (int i = 0; i < n.u.list->num; ++i)
            list.append(nodeToVariant(n.u.list->values[i]));
        return list;
    }
    default: return {};
    }
}

} // namespace

// Renders on Qt's render thread into the item's framebuffer.
class MpvRenderer : public QQuickFramebufferObject::Renderer {
public:
    MpvRenderer(std::shared_ptr<mpv_handle> mpv, std::shared_ptr<MpvShared> shared)
        : m_mpv(std::move(mpv)), m_shared(std::move(shared)) {}
    ~MpvRenderer() override {
        // Freed before the handle (the shared_ptr keeps it alive until here).
        if (m_render)
            mpv_render_context_free(m_render);
    }

    QOpenGLFramebufferObject *createFramebufferObject(const QSize &size) override {
        if (!m_render) {
            mpv_opengl_init_params gl{procAddress, nullptr};
            // No MPV_RENDER_PARAM_ADVANCED_CONTROL: with it, mpv's core waits for the render
            // thread, which Qt's threaded render loop blocks while the GUI thread waits on mpv,
            // and the whole window froze a few seconds into playback.
            QList<mpv_render_param> params{
                {MPV_RENDER_PARAM_API_TYPE, const_cast<char *>(MPV_RENDER_API_TYPE_OPENGL)},
                {MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &gl},
            };
            // The display connection lets hardware decoding hand frames to OpenGL without a copy.
            // Windows (desktop OpenGL) and macOS (3.2 core, see main.cpp) need none.
#if TORRENTPLAYER_FREEDESKTOP
#if QT_CONFIG(wayland)
            if (auto *wayland = qGuiApp->nativeInterface<QNativeInterface::QWaylandApplication>())
                params.append({MPV_RENDER_PARAM_WL_DISPLAY, wayland->display()});
#endif
#if QT_CONFIG(xcb)
            if (auto *x11 = qGuiApp->nativeInterface<QNativeInterface::QX11Application>())
                params.append({MPV_RENDER_PARAM_X11_DISPLAY, x11->display()});
#endif
#endif
            params.append({MPV_RENDER_PARAM_INVALID, nullptr});
            if (mpv_render_context_create(&m_render, m_mpv.get(), params.data()) < 0) {
                qWarning("mpv: could not create the OpenGL render context");
                m_render = nullptr;
            } else {
                mpv_render_context_set_update_callback(m_render, renderUpdate, m_shared.get());
                m_shared->post("onRenderReady");
            }
        }
        return QQuickFramebufferObject::Renderer::createFramebufferObject(size);
    }

    void render() override {
        if (!m_render)
            return;
        QOpenGLFramebufferObject *target = framebufferObject();
        mpv_opengl_fbo fbo{int(target->handle()), target->width(), target->height(), 0};
        int flip = 0;
        mpv_render_param params[] = {
            {MPV_RENDER_PARAM_OPENGL_FBO, &fbo},
            {MPV_RENDER_PARAM_FLIP_Y, &flip},
            {MPV_RENDER_PARAM_INVALID, nullptr},
        };
        // Qt's own GL state is saved around mpv's drawing.
        if (m_window)
            m_window->beginExternalCommands();
        mpv_render_context_render(m_render, params);
        if (m_window)
            m_window->endExternalCommands();
    }

    void synchronize(QQuickFramebufferObject *item) override { m_window = item->window(); }

private:
    std::shared_ptr<mpv_handle> m_mpv;
    std::shared_ptr<MpvShared> m_shared;
    mpv_render_context *m_render = nullptr;
    QQuickWindow *m_window = nullptr;
};

MpvPlayer::MpvPlayer(QQuickItem *parent) : QQuickFramebufferObject(parent), m_shared(std::make_shared<MpvShared>()) {
    setMirrorVertically(true);
    m_shared->item = this;

    // libmpv refuses to start unless numbers parse the C way.
    std::setlocale(LC_NUMERIC, "C");
    mpv_handle *mpv = mpv_create();
    if (!mpv) {
        m_error = tr("Could not start mpv");
        return;
    }
    m_mpv = std::shared_ptr<mpv_handle>(mpv, mpv_terminate_destroy);

    // As the Android player (PlayerActivity.initMpv), for a desktop GPU.
    const QList<QPair<const char *, const char *>> options = {
        {"vo", "libmpv"},
        {"hwdec", "auto-safe"},
        {"profile", "fast"},
        {"vd-lavc-film-grain", "cpu"},
        {"tls-verify", "no"},
        {"cache", "yes"},
        // The engine already holds the data; mpv needs only a short read-ahead of its own.
        {"demuxer-max-bytes", "32MiB"},
        {"demuxer-max-back-bytes", "16MiB"},
        {"hr-seek", "no"},
        {"hr-seek-framedrop", "yes"},
        {"cache-pause-initial", "no"},
        {"network-timeout", "0"},
        {"keep-open", "yes"},
        {"idle", "yes"},
        {"save-position-on-quit", "no"},
        // Keys are the app's.
        {"input-default-bindings", "no"},
        {"input-vo-keyboard", "no"},
        {"osc", "no"},
        {"terminal", "no"},
        {"config", "no"},
        // Subtitles start off; the menu turns them on.
        {"sid", "no"},
        {"sub-auto", "no"},
        {"sub-scale-with-window", "yes"},
        {"osd-level", "0"},
        {"screenshot-format", "jpg"},
        {"screenshot-jpeg-quality", "85"},
        {"audio-client-name", "torrentplayer"},
    };
    for (const auto &option : options)
        mpv_set_option_string(mpv, option.first, option.second);
    // For trying the player without a display or sound: "vo=null,ao=null".
    for (const QString &pair : qEnvironmentVariable("TORRENTPLAYER_MPV_OPTIONS").split(',', Qt::SkipEmptyParts)) {
        const QString name = pair.section('=', 0, 0).trimmed();
        mpv_set_option_string(mpv, name.toUtf8().constData(), pair.section('=', 1).trimmed().toUtf8().constData());
        if (name == "vo")
            m_renderWaitOver = true;
    }
    mpv_request_log_messages(mpv, "info");
    if (mpv_initialize(mpv) < 0) {
        m_error = tr("Could not start mpv");
        m_mpv.reset();
        return;
    }
    mpv_observe_property(mpv, 0, "time-pos", MPV_FORMAT_DOUBLE);
    mpv_observe_property(mpv, 0, "duration", MPV_FORMAT_DOUBLE);
    mpv_observe_property(mpv, 0, "pause", MPV_FORMAT_FLAG);
    mpv_observe_property(mpv, 0, "paused-for-cache", MPV_FORMAT_FLAG);
    mpv_observe_property(mpv, 0, "seeking", MPV_FORMAT_FLAG);
    mpv_observe_property(mpv, 0, "seekable", MPV_FORMAT_FLAG);
    mpv_observe_property(mpv, 0, "eof-reached", MPV_FORMAT_FLAG);
    mpv_observe_property(mpv, 0, "demuxer-cache-time", MPV_FORMAT_DOUBLE);
    mpv_observe_property(mpv, 0, "volume", MPV_FORMAT_DOUBLE);
    mpv_observe_property(mpv, 0, "mute", MPV_FORMAT_FLAG);
    mpv_observe_property(mpv, 0, "speed", MPV_FORMAT_DOUBLE);
    mpv_observe_property(mpv, 0, "video-params/aspect", MPV_FORMAT_DOUBLE);
    mpv_observe_property(mpv, 0, "track-list", MPV_FORMAT_NODE);
    mpv_observe_property(mpv, 0, "aid", MPV_FORMAT_STRING);
    mpv_observe_property(mpv, 0, "sid", MPV_FORMAT_STRING);
    mpv_set_wakeup_callback(mpv, wakeup, m_shared.get());

    m_stallTimer = new QTimer(this);
    m_stallTimer->setInterval(1000);
    connect(m_stallTimer, &QTimer::timeout, this, &MpvPlayer::checkStall);
    m_stallTimer->start();
}

// mpv's clock follows the sound card. When the audio output takes no data (PipeWire
// left with only a silent "Dummy Output", a Bluetooth headset that dropped), playback
// sits still forever without any error. After a few seconds of a clock that does not
// move while nothing waits for data, play on without sound and say why.
void MpvPlayer::checkStall() {
    // Data must already be waiting ahead of the clock: a stall while the torrent is still
    // fetching is buffering, not a dead sound card.
    const bool dataAhead = m_cacheEnd > m_position + 2000;
    if (!m_mpv || !m_loaded || m_paused || m_seeking || m_pausedForCache || m_eof || m_audioFallback || !dataAhead) {
        m_stallPosition = -1;
        m_stallSeconds = 0;
        return;
    }
    if (m_position != m_stallPosition) {
        m_stallPosition = m_position;
        m_stallSeconds = 0;
        return;
    }
    if (++m_stallSeconds < 4)
        return;
    m_audioFallback = true;
    qWarning("mpv: playback clock stuck with audio on; switching to ao=null");
    setProperty("ao", "null");
    command({"ao-reload"});
    emit notice(tr("Your sound output isn't playing anything, so the video continues without sound. "
                   "Check your speakers or headphones, then open the video again."));
}

MpvPlayer::~MpvPlayer() {
    {
        std::lock_guard<std::mutex> guard(m_shared->lock);
        m_shared->item = nullptr;
    }
    if (m_mpv) {
        mpv_set_wakeup_callback(m_mpv.get(), nullptr, nullptr);
        const char *args[] = {"stop", nullptr};
        mpv_command(m_mpv.get(), args);
    }
}

QQuickFramebufferObject::Renderer *MpvPlayer::createRenderer() const {
    if (!m_mpv)
        return nullptr;
    window()->setPersistentGraphics(true);
    return new MpvRenderer(m_mpv, m_shared);
}

QString MpvPlayer::trackLabel(const QString &title, const QString &lang, int number) {
    QStringList parts;
    if (!title.isEmpty())
        parts << title;
    if (!lang.isEmpty())
        parts << lang;
    return parts.isEmpty() ? tr("Track %1").arg(number) : parts.join(" · ");
}

// ---------------------------------------------------------------- setters

void MpvPlayer::setSource(const QString &source) {
    if (source == m_source)
        return;
    m_source = source;
    emit sourceChanged();
    // After the current binding pass, so startMs set alongside it is in.
    QMetaObject::invokeMethod(this, "loadSource", Qt::QueuedConnection);
}

void MpvPlayer::setStartMs(qint64 ms) {
    if (ms == m_startMs)
        return;
    m_startMs = ms;
    emit startMsChanged();
}

void MpvPlayer::onRenderReady() {
    qInfo("mpv: OpenGL render context ready");
    m_renderReady = true;
    if (m_loadPending)
        loadSource();
}

void MpvPlayer::loadSource() {
    if (!m_mpv)
        return;
    // mpv opens its video output with the file; without the render context by then the
    // file plays with no picture. Wait for it, but not forever (no OpenGL, offscreen).
    if (!m_renderReady && !m_source.isEmpty() && !m_renderWaitOver) {
        if (!m_loadPending) {
            m_loadPending = true;
            QTimer::singleShot(3000, this, [this] {
                m_renderWaitOver = true;
                if (m_loadPending) {
                    qWarning("mpv: no OpenGL render context after 3 s; playing without a picture");
                    loadSource();
                }
            });
        }
        return;
    }
    m_loadPending = false;
    m_loaded = false;
    m_eof = false;
    m_error.clear();
    emit errorStringChanged();
    emit bufferingChanged();
    if (m_source.isEmpty()) {
        command({"stop"});
        return;
    }
    setProperty("start", m_startMs > 1000 ? QString::number(m_startMs / 1000.0, 'f', 3) : QStringLiteral("none"));
    command({"loadfile", m_source, "replace"});
}

void MpvPlayer::setPaused(bool paused) { setProperty("pause", paused ? "yes" : "no"); }
void MpvPlayer::play() { setPaused(false); }
void MpvPlayer::pause() { setPaused(true); }
void MpvPlayer::stop() { command({"stop"}); }

void MpvPlayer::setVolume(double volume) {
    setProperty("volume", QString::number(qBound(0.0, volume, 1.0) * 100.0, 'f', 1));
}
void MpvPlayer::setMuted(bool muted) { setProperty("mute", muted ? "yes" : "no"); }
void MpvPlayer::setSpeed(double speed) { setProperty("speed", QString::number(speed, 'f', 3)); }
void MpvPlayer::setAudioId(int id) { setProperty("aid", id < 0 ? QStringLiteral("no") : QString::number(id)); }
void MpvPlayer::setSubtitleId(int id) { setProperty("sid", id < 0 ? QStringLiteral("no") : QString::number(id)); }

bool MpvPlayer::buffering() const {
    return !m_source.isEmpty() && m_error.isEmpty() && (!m_loaded || m_pausedForCache || m_seeking);
}

void MpvPlayer::seek(qint64 ms, bool exact) {
    command({"seek", QString::number(qMax<qint64>(0, ms) / 1000.0, 'f', 3), exact ? "absolute+exact" : "absolute+keyframes"});
}

void MpvPlayer::frameStep() { command({"frame-step"}); }

void MpvPlayer::addSubtitle(const QString &pathOrUrl, const QString &title) {
    QStringList args{"sub-add", pathOrUrl, "select"};
    if (!title.isEmpty())
        args << title;
    command(args);
}

void MpvPlayer::screenshotTo(const QString &path) {
    const quint64 id = ScreenshotBase + m_nextReply++;
    m_screenshots.insert(id, path);
    command({"screenshot-to-file", path, "video"}, id);
}

// ---------------------------------------------------------------- mpv

void MpvPlayer::setProperty(const char *name, const QString &value) {
    if (m_mpv)
        mpv_set_property_string(m_mpv.get(), name, value.toUtf8().constData());
}

void MpvPlayer::command(const QStringList &args, quint64 userdata) {
    if (!m_mpv)
        return;
    QList<QByteArray> utf8;
    for (const QString &arg : args)
        utf8 << arg.toUtf8();
    std::vector<const char *> argv;
    for (const QByteArray &arg : std::as_const(utf8))
        argv.push_back(arg.constData());
    argv.push_back(nullptr);
    // Async: sub-add of a URL or a seek must not block the window.
    mpv_command_async(m_mpv.get(), userdata, argv.data());
}

void MpvPlayer::onMpvEvents() {
    if (!m_mpv)
        return;
    while (true) {
        mpv_event *event = mpv_wait_event(m_mpv.get(), 0);
        if (event->event_id == MPV_EVENT_NONE)
            break;
        handleEvent(event);
    }
}

void MpvPlayer::handleEvent(mpv_event *event) {
    switch (event->event_id) {
    case MPV_EVENT_PROPERTY_CHANGE: {
        auto *p = static_cast<mpv_event_property *>(event->data);
        handleProperty(p->name, p->format, p->data);
        break;
    }
    case MPV_EVENT_LOG_MESSAGE: {
        auto *m = static_cast<mpv_event_log_message *>(event->data);
        const QString text = QString::fromUtf8(m->text).trimmed();
        if (m->log_level <= MPV_LOG_LEVEL_WARN)
            qWarning().noquote() << "mpv:" << m->prefix << text;
        else
            qInfo().noquote() << "mpv:" << m->prefix << text;
        break;
    }
    case MPV_EVENT_FILE_LOADED:
        m_loaded = true;
        emit bufferingChanged();
        emit fileLoaded();
        break;
    case MPV_EVENT_PLAYBACK_RESTART:
        if (!m_loaded) {
            m_loaded = true;
            emit fileLoaded();
        }
        emit bufferingChanged();
        break;
    case MPV_EVENT_END_FILE: {
        auto *end = static_cast<mpv_event_end_file *>(event->data);
        if (end->reason == MPV_END_FILE_REASON_ERROR) {
            m_error = QString::fromUtf8(mpv_error_string(end->error));
            emit errorStringChanged();
            emit bufferingChanged();
        }
        break;
    }
    case MPV_EVENT_COMMAND_REPLY: {
        if (m_screenshots.contains(event->reply_userdata)) {
            const QString path = m_screenshots.take(event->reply_userdata);
            emit screenshotSaved(path, event->error >= 0);
        }
        break;
    }
    default:
        break;
    }
}

void MpvPlayer::handleProperty(const char *name, int format, void *data) {
    const QByteArray key(name);
    const bool has = format != MPV_FORMAT_NONE && data;
    auto asDouble = [&] { return has && format == MPV_FORMAT_DOUBLE ? *static_cast<double *>(data) : 0.0; };
    auto asFlag = [&] { return has && format == MPV_FORMAT_FLAG ? *static_cast<int *>(data) != 0 : false; };
    auto asString = [&] { return has && format == MPV_FORMAT_STRING ? QString::fromUtf8(*static_cast<char **>(data)) : QString(); };

    if (key == "video-params/aspect") {
        const double v = asDouble() > 0 ? asDouble() : 0.0;
        if (v != m_videoAspect) { m_videoAspect = v; emit videoAspectChanged(); }
    } else if (key == "time-pos") {
        const qint64 v = qint64(asDouble() * 1000);
        if (v != m_position) { m_position = v; emit positionChanged(); }
    } else if (key == "duration") {
        const qint64 v = qint64(asDouble() * 1000);
        if (v != m_duration) { m_duration = v; emit durationChanged(); }
    } else if (key == "pause") {
        if (asFlag() != m_paused) { m_paused = asFlag(); emit pausedChanged(); }
    } else if (key == "paused-for-cache") {
        m_pausedForCache = asFlag();
        emit bufferingChanged();
    } else if (key == "seeking") {
        m_seeking = asFlag();
        emit bufferingChanged();
    } else if (key == "seekable") {
        if (asFlag() != m_seekable) { m_seekable = asFlag(); emit seekableChanged(); }
    } else if (key == "eof-reached") {
        if (asFlag() != m_eof) { m_eof = asFlag(); emit eofChanged(); }
    } else if (key == "demuxer-cache-time") {
        const qint64 v = qint64(asDouble() * 1000);
        if (v != m_cacheEnd) { m_cacheEnd = v; emit cacheEndMsChanged(); }
    } else if (key == "volume") {
        if (has) { m_volume = asDouble() / 100.0; emit volumeChanged(); }
    } else if (key == "mute") {
        if (asFlag() != m_muted) { m_muted = asFlag(); emit mutedChanged(); }
    } else if (key == "speed") {
        if (has) { m_speed = asDouble(); emit speedChanged(); }
    } else if (key == "track-list") {
        readTracks(has && format == MPV_FORMAT_NODE ? data : nullptr);
    } else if (key == "aid" || key == "sid") {
        bool ok = false;
        const int id = asString().toInt(&ok);
        const int value = ok ? id : -1;
        if (key == "aid" && value != m_audioId) { m_audioId = value; emit audioIdChanged(); }
        if (key == "sid" && value != m_subtitleId) { m_subtitleId = value; emit subtitleIdChanged(); }
    }
}

void MpvPlayer::readTracks(void *data) {
    m_audioTracks.clear();
    m_subtitleTracks.clear();
    const QVariantList list = data ? nodeToVariant(*static_cast<mpv_node *>(data)).toList() : QVariantList();
    int audio = 0, subs = 0;
    for (const QVariant &value : list) {
        const QVariantMap t = value.toMap();
        const QString type = t.value("type").toString();
        if (type != "audio" && type != "sub")
            continue;
        const int number = type == "audio" ? ++audio : ++subs;
        QVariantMap track{
            {"id", t.value("id").toInt()},
            {"title", t.value("title").toString()},
            {"lang", t.value("lang").toString()},
            {"codec", t.value("codec").toString()},
            {"external", t.value("external").toBool()},
            {"externalFilename", t.value("external-filename").toString()},
            {"label", trackLabel(t.value("title").toString(), t.value("lang").toString(), number)},
        };
        (type == "audio" ? m_audioTracks : m_subtitleTracks).append(track);
    }
    emit tracksChanged();
}
