# Test fixtures

`tiny.dat` and `tiny.torrent` are the original small engine fixtures.

`playback.mp4` is a generated two-second test pattern and 440 Hz tone (H.264/AAC). It contains no third-party media and is used for local peer transfer and decoder regression tests. It is never packaged in the APK.

Generate it with:

```sh
ffmpeg -f lavfi -i 'testsrc2=size=160x90:rate=12' \
  -f lavfi -i 'sine=frequency=440:sample_rate=44100' -t 2 \
  -c:v libx264 -preset fast -crf 32 -pix_fmt yuv420p \
  -c:a aac -b:a 32k -movflags +faststart playback.mp4
```

`jni/webtor/app/EngineHost.java` drives the real native exports under host JVM CheckJNI. Run `bash scripts/test-jni.sh` from the repository root with a JDK installed.
