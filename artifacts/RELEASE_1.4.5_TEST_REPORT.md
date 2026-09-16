# 1.4.5 resume recovery validation

Date: 2026-09-16

## Reported failure and regression

The released `/configure` handler hashed selected saved pieces while holding the
torrent record lock, using the JNI request's 60-second context. Slow/large restores
failed and blocked pause/status during the check.

The new test gates storage reads until after a shortened request deadline. Running
it against the original 1.4.4 handler reproduces
`verify saved data: context deadline exceeded`. With the fix configuration returns
promptly, checking survives request expiry and status/pause/resume remain usable.

Additional tests cover cancellation on removal and shutdown, reopening a removed
torrent, and restart with a mixture of valid, corrupt and missing data. A real
local TCP seed sends exactly the bytes missing after verification; the destination
matches the seed byte for byte. Previously verified data is not downloaded again.

## Passed

- 26 Go tests: `go test -tags=nosqlite,noboltdb -race -count=1 ./...`.
- `go vet -tags=nosqlite,noboltdb ./...`.
- JNI CheckJNI smoke: control, duplicated descriptors, UTF-8 paths, events,
  pause/resume, playback, seek and shutdown.
- 14 core and 30 app unit tests, including checking status parsing, completion
  suppression and responsive pause controls during checking.
- Android release lint, minification/resource shrinking and APK assembly.
- APK version/package, ZIP integrity and signature verification.

## APK

- Version 1.4.5 (22), `webtor.app`, arm64-v8a.
- Size: 8,800,309 bytes.
- SHA-256: `30fb2cde91b4ee738037118948b8ee9d211c5ff21026751d8b9f05036cff4774`.
- Signing certificate SHA-256:
  `9c33627be30850a9fd315b374896b5fe229bd8c09c51e0d7a77583cd6d3a38f3`.
  Same certificate as the previous phone/GitHub APK.

## Limits

The automated restart test creates a fresh engine with no in-memory completion
state and reopens the saved file. Device force-stop/reboot testing of this APK has
not been performed. Large files still require disk verification; this fix removes
the request timeout and control blocking, not the integrity check.
