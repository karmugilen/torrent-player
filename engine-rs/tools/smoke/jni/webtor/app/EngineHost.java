package webtor.app;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.Base64;
import java.util.HexFormat;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.regex.Pattern;

/** Public-swarm calls through Android's actual JNI ABI; no synthetic payload. */
public final class EngineHost {
    private native int startEngine(byte[] path, int maxPeers);
    private native byte[] requestEngine(String method, String path, byte[] body);
    private native long waitForEngineEvent(long version, long timeout);
    private native long openPlayback(String id, int index);
    private native int readPlayback(long handle, long offset, int size, byte[] target);
    private native void closePlayback(long handle);
    private native long openPreview(String id, int index);
    private native int readPreview(long handle, long offset, int size, byte[] target);
    private native void closePreview(long handle);

    private static String quote(String value) {
        return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"")
                .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t") + "\"";
    }

    private static void emit(String body) {
        System.out.println(body);
        System.out.flush();
    }

    private static double seconds(long start) {
        return (System.nanoTime() - start) / 1_000_000_000.0;
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new IllegalStateException(message);
    }

    private String request(String method, String path, String body) {
        long start = System.nanoTime();
        String framed = new String(requestEngine(method, path, body.getBytes(StandardCharsets.UTF_8)),
                StandardCharsets.UTF_8);
        int newline = framed.indexOf('\n');
        check(newline > 0, "invalid JNI control frame");
        int status = Integer.parseInt(framed.substring(0, newline));
        String response = framed.substring(newline + 1);
        emit("{\"kind\":\"control\",\"method\":" + quote(method) + ",\"path\":" + quote(path)
                + ",\"status\":" + status + ",\"seconds\":" + seconds(start) + ",\"response\":" + response + "}");
        check(status >= 200 && status < 300, method + " " + path + " returned " + status + " " + response);
        return response;
    }

    private static String stringField(String json, String name) {
        var matcher = Pattern.compile("\"" + name + "\"\\s*:\\s*\"([^\"]+)\"").matcher(json);
        check(matcher.find(), "missing string field " + name);
        return matcher.group(1);
    }

    private void read(long handle, long offset, int size, boolean preview) throws Exception {
        byte[] bytes = new byte[size];
        long start = System.nanoTime();
        int count = preview ? readPreview(handle, offset, size, bytes) : readPlayback(handle, offset, size, bytes);
        String hash = count > 0 ? HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256")
                .digest(java.util.Arrays.copyOf(bytes, count))) : "";
        emit("{\"kind\":" + quote(preview ? "preview-read" : "playback-read") + ",\"offset\":" + offset
                + ",\"count\":" + count + ",\"seconds\":" + seconds(start) + ",\"sha256\":" + quote(hash) + "}");
        check(count == size, "short/failed native read: " + count + " expected " + size);
    }

    private void run(Path directory, String magnet, int fileCount, int index, long length) throws Exception {
        Files.createDirectories(directory);
        check(startEngine(directory.toString().getBytes(StandardCharsets.UTF_8), 55) == 0, "startEngine failed");
        request("POST", "/settings", "{\"downloadRateBytesSec\":524288}");
        request("POST", "/settings", "{\"ramCacheBytes\":33554432}");
        request("POST", "/settings", "{\"payloadTransfersAllowed\":true}");
        request("POST", "/settings", "{\"seedingEnabled\":false}");
        long start = System.nanoTime();
        String added = request("POST", "/add", "{\"torrentId\":" + quote(magnet) + ",\"prepare\":true}");
        String id = stringField(added, "id");
        String identity = "\"id\":" + quote(id);
        long version = -1;
        while (true) {
            String status = request("GET", "/torrent/" + id, "");
            if (Pattern.compile("\"ready\"\\s*:\\s*true").matcher(status).find()) break;
            check(seconds(start) < 90, "metadata timeout after 90 seconds");
            version = waitForEngineEvent(version, 1000);
        }
        emit("{\"kind\":\"metadata\",\"seconds\":" + seconds(start) + "}");

        // ThumbnailRepository's 10-second deadline closes the native handle
        // concurrently with a blocked read; use the exact same mechanism.
        long preview = openPreview(id, index);
        check(preview > 0, "openPreview returned zero");
        ScheduledExecutorService deadlines = Executors.newSingleThreadScheduledExecutor();
        var deadline = deadlines.schedule(() -> closePreview(preview), 10, TimeUnit.SECONDS);
        try {
            read(preview, 0, 16384, true);
            read(preview, Math.max(0, length - 16384), 16384, true);
        } finally {
            deadline.cancel(false);
            closePreview(preview);
            deadlines.shutdownNow();
        }
        request("GET", "/metadata/" + id, "");
        String descriptors = "[" + String.join(",", java.util.Collections.nCopies(fileCount, "null")) + "]";
        request("POST", "/configure", "{" + identity + ",\"descriptors\":" + descriptors
                + ",\"selected\":[" + index + "],\"storageMode\":\"stream\",\"streamCacheBytes\":33554432}");
        request("POST", "/play", "{" + identity + ",\"fileIndex\":" + index + ",\"offset\":0,\"boost\":true,\"adaptiveBuffer\":true}");
        long playback = openPlayback(id, index);
        check(playback > 0, "openPlayback returned zero");
        ScheduledExecutorService playbackDeadline = Executors.newSingleThreadScheduledExecutor();
        var playbackTimeout = playbackDeadline.schedule(() -> closePlayback(playback), 120, TimeUnit.SECONDS);
        try {
            read(playback, 0, 65536, false);
            for (double fraction : new double[] { .05, .25, .5, .75, .95 }) {
                read(playback, (long) ((length - 65536) * fraction), 65536, false);
            }
        } finally {
            playbackTimeout.cancel(false);
            closePlayback(playback);
            playbackDeadline.shutdownNow();
        }
        request("POST", "/pause", "{" + identity + "}");
        request("POST", "/resume", "{" + identity + "}");
        request("POST", "/remove", "{" + identity + ",\"destroyStore\":true}");
        emit("{\"kind\":\"result\",\"passed_native_flow\":true}");
    }

    public static void main(String[] args) {
        try {
            check(args.length == 6, "expected library directory magnetBase64 fileCount index length");
            System.load(Path.of(args[0]).toAbsolutePath().toString());
            String magnet = new String(Base64.getDecoder().decode(args[2]), StandardCharsets.UTF_8);
            new EngineHost().run(Path.of(args[1]), magnet, Integer.parseInt(args[3]),
                    Integer.parseInt(args[4]), Long.parseLong(args[5]));
        } catch (Throwable error) {
            emit("{\"kind\":\"result\",\"passed_native_flow\":false,\"error\":" + quote(error.toString()) + "}");
            error.printStackTrace(System.err);
            System.exit(1);
        }
    }
}
