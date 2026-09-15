package webtor.app;

import java.io.RandomAccessFile;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Base64;
import java.util.regex.Pattern;

/** Exercises the real cgo exports under CheckJNI without an Android device. */
public class EngineHost {
    private native int startEngine(byte[] path, int maxPeers);
    private native byte[] requestEngine(String method, String path, byte[] body);
    private native long waitForEngineEvent(long version, long timeout);
    private native long openPlayback(String id, int index);
    private native int readPlayback(long handle, long offset, int size, byte[] target);
    private native void closePlayback(long handle);

    private String request(String method, String path, String body, int status) {
        String response = new String(requestEngine(method, path, body.getBytes(StandardCharsets.UTF_8)), StandardCharsets.UTF_8);
        check(response.startsWith(status + "\n"), "unexpected response: " + response);
        return response.substring(response.indexOf('\n') + 1);
    }

    private static void check(boolean ok, String message) {
        if (!ok) throw new AssertionError(message);
    }

    public static void main(String[] args) throws Exception {
        Path testDir = Path.of(args[0]);
        Path fixtures = Path.of(args[1]);
        System.load(testDir.resolve("libengine.so").toString());
        EngineHost engine = new EngineHost();
        engine.request("GET", "/stats", "", 503);
        Path dataDir = testDir.resolve("தமிழ்-🎬");
        check(engine.startEngine(dataDir.toString().getBytes(StandardCharsets.UTF_8), 8) == 0, "startup failed");
        String stats = engine.request("GET", "/stats", "", 200);
        check(stats.contains(dataDir.toString()), "UTF-8 path was corrupted");
        check(stats.contains("\"ctlPort\":0"), "Android opened a control listener");
        engine.request("POST", "/add", "{}", 400);
        String metadata = Base64.getEncoder().encodeToString(Files.readAllBytes(fixtures.resolve("tiny.torrent")));
        String added = engine.request("POST", "/add", "{\"prepare\":true,\"torrentData\":\""+metadata+"\"}", 200);
        var matcher = Pattern.compile("\"id\":\"([^\"]+)\"").matcher(added);
        check(matcher.find(), "missing torrent ID");
        String id = matcher.group(1);
        byte[] wanted = Files.readAllBytes(fixtures.resolve("tiny.dat"));
        Path saved = dataDir.resolve("tiny.dat");
        Files.write(saved, wanted);
        try (RandomAccessFile file = new RandomAccessFile(saved.toFile(), "rw")) {
            Field field = java.io.FileDescriptor.class.getDeclaredField("fd");
            field.setAccessible(true);
            int fd = field.getInt(file.getFD());
            engine.request("POST", "/configure", "{\"id\":\""+id+"\",\"descriptors\":["+fd+"],\"selected\":[0]}", 200);
        }
        long version = engine.waitForEngineEvent(-1, 1000);
        engine.request("POST", "/pause", "{\"id\":\""+id+"\"}", 200);
        check(engine.waitForEngineEvent(version, 1000) != version, "pause event missing");
        check(engine.request("GET", "/torrent/"+id, "", 200).contains("\"paused\":true"), "pause status wrong");
        engine.request("POST", "/resume", "{\"id\":\""+id+"\"}", 200);
        check(engine.openPlayback(id, 99) == 0, "invalid file opened");
        long handle = engine.openPlayback(id, 0);
        check(handle > 0, "reader did not open");
        byte[] got = new byte[wanted.length + 16];
        int count = engine.readPlayback(handle, 0, got.length, got);
        check(count == wanted.length && Arrays.equals(wanted, Arrays.copyOf(got, count)), "native media read mismatch");
        check(engine.readPlayback(handle, wanted.length, 8, got) == 0, "incorrect EOF");
        int suffix = Math.min(wanted.length, 4);
        check(engine.readPlayback(handle, wanted.length-suffix, suffix, got) == suffix, "seek failed");
        check(Arrays.equals(Arrays.copyOf(got, suffix), Arrays.copyOfRange(wanted, wanted.length-suffix, wanted.length)), "seek bytes wrong");
        engine.closePlayback(handle);
        check(engine.readPlayback(handle, 0, 1, got) == -1, "closed reader still readable");
        engine.request("POST", "/remove", "{\"id\":\""+id+"\"}", 200);
        engine.request("GET", "/torrent/"+id, "", 404);
        engine.request("POST", "/shutdown", "{}", 200);
        System.out.println("JNI smoke passed: UTF-8, control, descriptors, events, pause/resume, playback, seeking, close");
    }
}
