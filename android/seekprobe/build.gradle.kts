// Test-only player for the emulator. It takes the stream the app hands to an
// external player and seeks around it like one, logging each seek's time to
// first bytes under the SeekProbe tag. Not shipped.
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "webtor.seekprobe"
    compileSdk = 34

    defaultConfig {
        applicationId = "webtor.seekprobe"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "1"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}
