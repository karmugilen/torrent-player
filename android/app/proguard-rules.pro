-keep class webtor.app.NodeHost { *; }
-keep class webtor.app.WebtorApp { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
-dontwarn okhttp3.internal.platform.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**
