//go:build android || jnitest

package main

/*
#include <jni.h>
#include <stdlib.h>
#define LOG_TAG "webtor-engine"
#ifdef __ANDROID__
#include <android/log.h>
#define ALOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define ALOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)
#else
#include <stdio.h>
#define ALOGI(...) fprintf(stderr, __VA_ARGS__)
#define ALOGE(...) fprintf(stderr, __VA_ARGS__)
#endif

static void log_info(const char* msg) {
    ALOGI("%s", msg);
}

static void log_error(const char* msg) {
    ALOGE("%s", msg);
}

static const char* get_string_utf(JNIEnv *env, jstring str) {
    if (str == NULL) return NULL;
    return (*env)->GetStringUTFChars(env, str, NULL);
}

static void release_string_utf(JNIEnv *env, jstring str, const char *chars) {
    if (str != NULL && chars != NULL) {
        (*env)->ReleaseStringUTFChars(env, str, chars);
    }
}

static jbyteArray new_byte_array(JNIEnv *env, const char *data, int length) {
    jbyteArray result = (*env)->NewByteArray(env, length);
    if (result != NULL && length > 0) {
        (*env)->SetByteArrayRegion(env, result, 0, length, (const jbyte*)data);
    }
    return result;
}

static int byte_array_length(JNIEnv *env, jbyteArray value) {
    return (*env)->GetArrayLength(env, value);
}

static void copy_byte_array(JNIEnv *env, jbyteArray value, char *data, int length) {
    (*env)->GetByteArrayRegion(env, value, 0, length, (jbyte*)data);
}

static void fill_byte_array(JNIEnv *env, jbyteArray value, char *data, int length) {
    (*env)->SetByteArrayRegion(env, value, 0, length, (jbyte*)data);
}
*/
import "C"

import (
	"fmt"
	"runtime"
	"sync"
	"sync/atomic"
	"time"
	"unsafe"
)

var androidEngine struct {
	sync.RWMutex
	server *EngineServer
}

var androidReaders sync.Map
var nextReaderID atomic.Int64

func androidLogI(msg string) {
	cMsg := C.CString(msg)
	defer C.free(unsafe.Pointer(cMsg))
	C.log_info(cMsg)
}

func androidLogE(msg string) {
	cMsg := C.CString(msg)
	defer C.free(unsafe.Pointer(cMsg))
	C.log_error(cMsg)
}

func javaString(env *C.JNIEnv, value C.jstring) string {
	chars := C.get_string_utf(env, value)
	if chars == nil {
		return ""
	}
	defer C.release_string_utf(env, value, chars)
	return C.GoString(chars)
}

func javaBytes(env *C.JNIEnv, value []byte) C.jbyteArray {
	if len(value) == 0 {
		return C.new_byte_array(env, nil, 0)
	}
	data := C.CBytes(value)
	defer C.free(data)
	return C.new_byte_array(env, (*C.char)(data), C.int(len(value)))
}

func fromJavaBytes(env *C.JNIEnv, value C.jbyteArray) string {
	length := C.byte_array_length(env, value)
	if length == 0 {
		return ""
	}
	data := C.malloc(C.size_t(length))
	defer C.free(data)
	C.copy_byte_array(env, value, (*C.char)(data), length)
	return C.GoStringN((*C.char)(data), length)
}

func currentAndroidEngine() *EngineServer {
	androidEngine.RLock()
	defer androidEngine.RUnlock()
	return androidEngine.server
}

func initializeAndroidEngine(downloadDir string, maxPeers int) int {
	androidEngine.Lock()
	defer androidEngine.Unlock()
	if androidEngine.server != nil {
		return 0
	}
	if downloadDir == "" {
		androidLogE("download directory is empty")
		return 1
	}
	if maxPeers <= 0 {
		maxPeers = 55
	}
	server, err := NewEngineServer(0, downloadDir, maxPeers)
	if err != nil {
		androidLogE(fmt.Sprintf("Failed to create engine: %v", err))
		return 1
	}
	if err := server.StartStreaming(); err != nil {
		server.Close()
		androidLogE(fmt.Sprintf("Failed to start media server: %v", err))
		return 1
	}
	androidEngine.server = server
	server.notifyChange()
	androidLogI(fmt.Sprintf("Engine ready: JNI control, streamPort=%d", server.streamPort))
	go func() {
		<-server.Done()
		androidEngine.Lock()
		if androidEngine.server == server {
			androidEngine.server = nil
		}
		androidEngine.Unlock()
	}()
	return 0
}

//export Java_webtor_app_EngineHost_startEngine
func Java_webtor_app_EngineHost_startEngine(env *C.JNIEnv, host C.jobject, downloadDir C.jbyteArray, maxPeers C.jint) C.jint {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	androidLogI("initializing native Go engine")
	return C.jint(initializeAndroidEngine(fromJavaBytes(env, downloadDir), int(maxPeers)))
}

//export Java_webtor_app_EngineHost_requestEngine
func Java_webtor_app_EngineHost_requestEngine(env *C.JNIEnv, host C.jobject, method C.jstring, path C.jstring, body C.jbyteArray) C.jbyteArray {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	server := currentAndroidEngine()
	if server == nil {
		return javaBytes(env, []byte("503\n{\"error\":\"engine is starting\"}"))
	}
	status, response := server.DispatchControl(
		javaString(env, method),
		javaString(env, path),
		fromJavaBytes(env, body),
	)
	payload := append([]byte(fmt.Sprintf("%d\n", status)), response...)
	return javaBytes(env, payload)
}

//export Java_webtor_app_EngineHost_waitForEngineEvent
func Java_webtor_app_EngineHost_waitForEngineEvent(env *C.JNIEnv, host C.jobject, afterVersion C.jlong, timeoutMillis C.jlong) C.jlong {
	server := currentAndroidEngine()
	if server == nil {
		time.Sleep(250 * time.Millisecond)
		return afterVersion
	}
	timeout := time.Duration(timeoutMillis) * time.Millisecond
	if timeout <= 0 || timeout > time.Minute {
		timeout = 30 * time.Second
	}
	return C.jlong(server.WaitForChange(uint64(afterVersion), timeout))
}

//export Java_webtor_app_EngineHost_openPlayback
func Java_webtor_app_EngineHost_openPlayback(env *C.JNIEnv, host C.jobject, id C.jstring, index C.jint) C.jlong {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	s := currentAndroidEngine()
	if s == nil {
		return 0
	}
	reader, err := s.OpenPlayback(javaString(env, id), int(index))
	if err != nil {
		androidLogE(err.Error())
		return 0
	}
	handle := nextReaderID.Add(1)
	androidReaders.Store(handle, reader)
	return C.jlong(handle)
}

//export Java_webtor_app_EngineHost_readPlayback
func Java_webtor_app_EngineHost_readPlayback(env *C.JNIEnv, host C.jobject, handle C.jlong, offset C.jlong, size C.jint, target C.jbyteArray) C.jint {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	value, ok := androidReaders.Load(int64(handle))
	if !ok || size < 0 || size > 1024*1024 || size > C.jint(C.byte_array_length(env, target)) {
		return -1
	}
	data := make([]byte, int(size))
	n, err := value.(*playbackReader).ReadAt(data, int64(offset))
	if err != nil {
		androidLogE(fmt.Sprintf("playback read: %v", err))
		return -1
	}
	if n > 0 {
		C.fill_byte_array(env, target, (*C.char)(unsafe.Pointer(&data[0])), C.int(n))
	}
	return C.jint(n)
}

//export Java_webtor_app_EngineHost_closePlayback
func Java_webtor_app_EngineHost_closePlayback(env *C.JNIEnv, host C.jobject, handle C.jlong) {
	if value, ok := androidReaders.LoadAndDelete(int64(handle)); ok {
		value.(*playbackReader).Close()
	}
}

func main() {}
