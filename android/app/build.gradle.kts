plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The Kotlin client and bundled daemon must come from the same checkout.
// Stage the engine before Android reads assets, including IDE builds.
// Hook merge*Assets too: depending only on preBuild races packaging.
val repoRoot = rootProject.projectDir.parentFile
val engineDir = repoRoot.resolve("engine")
val syncEngine by tasks.registering(Exec::class) {
    group = "build"
    description = "Bundle the current torrent engine and Android-compatible dependencies"
    workingDir(repoRoot)
    commandLine("bash", repoRoot.resolve("scripts/sync-engine.sh").absolutePath)
    inputs.files(
        engineDir.resolve("main.js"),
        engineDir.resolve("protocol.js"),
        engineDir.resolve("document-store.js"),
        engineDir.resolve("package.json"),
        engineDir.resolve("package-lock.json"),
        engineDir.resolve("scripts/patch-webtorrent.mjs"),
        repoRoot.resolve("scripts/sync-engine.sh"),
    )
    outputs.file(file("src/main/assets/nodejs-project/main.js"))
}

tasks.named("preBuild") {
    dependsOn(syncEngine)
}

tasks.configureEach {
    if (name.startsWith("merge") && name.endsWith("Assets")) {
        dependsOn(syncEngine)
    }
}

android {
    namespace = "webtor.app"
    compileSdk = 34
    ndkVersion = "26.1.10909125"

    defaultConfig {
        applicationId = "webtor.app"
        minSdk = 26
        targetSdk = 34
        versionCode = 5
        versionName = "1.2.2"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            abiFilters.add("arm64-v8a")
        }
        externalNativeBuild {
            cmake {
                arguments += listOf("-DANDROID_STL=c++_shared")
            }
        }
    }

    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = signingConfigs.getByName("debug")
        }
        debug {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    androidResources {
        noCompress += listOf("node")
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
        }
    }
}

dependencies {
    implementation(project(":core"))
    testImplementation("junit:junit:4.13.2")

    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)

    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")

    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")

    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
