#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JNI_TEST_JAVA_HOME="${JAVA_HOME:-$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")}"
TEST_DIR="$(mktemp -d /tmp/webtor-jni.XXXXXX)"
cd "$ROOT/engine-go"
CGO_CFLAGS="-I$JNI_TEST_JAVA_HOME/include -I$JNI_TEST_JAVA_HOME/include/linux" \
  go build -tags jnitest,nosqlite,noboltdb -buildmode=c-shared -o "$TEST_DIR/libengine.so" .
"$JNI_TEST_JAVA_HOME/bin/javac" -d "$TEST_DIR" testdata/jni/webtor/app/EngineHost.java
"$JNI_TEST_JAVA_HOME/bin/java" -Xcheck:jni --add-opens java.base/java.io=ALL-UNNAMED \
  -cp "$TEST_DIR" webtor.app.EngineHost "$TEST_DIR" "$ROOT/engine-go/testdata"
