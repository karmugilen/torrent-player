-keep class webtor.app.EngineHost { *; }
-keep class webtor.app.WebtorApp { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**

# libmpv calls back into these from native code.
-keep class dev.jdtech.mpv.** { *; }
