#include <jni.h>
#include <string>
#include <cstdlib>
#include <cstring>
#include <unistd.h>
#include <android/log.h>
#include "node.h"

#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, "webtor-node", __VA_ARGS__)

extern "C"
JNIEXPORT jint JNICALL
Java_webtor_app_NodeHost_startNodeWithArguments(
        JNIEnv *env,
        jobject /* this */,
        jobjectArray arguments,
        jobjectArray environPairs,
        jstring cwdPath) {

    if (cwdPath != nullptr) {
        const char *cwd = env->GetStringUTFChars(cwdPath, nullptr);
        if (chdir(cwd) != 0) {
            LOGI("chdir failed: %s", cwd);
        }
        env->ReleaseStringUTFChars(cwdPath, cwd);
    }

    if (environPairs != nullptr) {
        jsize env_count = env->GetArrayLength(environPairs);
        for (int i = 0; i < env_count; i++) {
            auto pair = (jstring) env->GetObjectArrayElement(environPairs, i);
            const char *kv = env->GetStringUTFChars(pair, nullptr);
            const char *eq = strchr(kv, '=');
            if (eq) {
                std::string key(kv, eq - kv);
                setenv(key.c_str(), eq + 1, 1);
            }
            env->ReleaseStringUTFChars(pair, kv);
            env->DeleteLocalRef(pair);
        }
    }

    jsize argument_count = env->GetArrayLength(arguments);
    int c_arguments_size = 0;
    for (int i = 0; i < argument_count; i++) {
        auto arg = (jstring) env->GetObjectArrayElement(arguments, i);
        const char *chars = env->GetStringUTFChars(arg, nullptr);
        c_arguments_size += strlen(chars) + 1;
        env->ReleaseStringUTFChars(arg, chars);
        env->DeleteLocalRef(arg);
    }

    char *args_buffer = (char *) calloc(c_arguments_size, sizeof(char));
    char **argv = (char **) malloc(sizeof(char *) * argument_count);
    char *current = args_buffer;
    for (int i = 0; i < argument_count; i++) {
        auto arg = (jstring) env->GetObjectArrayElement(arguments, i);
        const char *chars = env->GetStringUTFChars(arg, nullptr);
        strncpy(current, chars, strlen(chars));
        argv[i] = current;
        current += strlen(chars) + 1;
        env->ReleaseStringUTFChars(arg, chars);
        env->DeleteLocalRef(arg);
    }

    LOGI("node::Start argc=%d", argument_count);
    int code = node::Start(argument_count, argv);
    LOGI("node::Start returned %d", code);
    free(argv);
    free(args_buffer);
    return code;
}
