plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val repoRoot = rootProject.projectDir.parentFile

// What the engine build reads: tests, docs and stray binaries do not rebuild it.
val engineSources = fileTree(repoRoot.resolve("engine-go")) {
    include("**/*.go", "**/go.mod", "**/go.sum", "**/*.c", "**/*.h", "build.sh")
    exclude("**/*_test.go", "**/testdata/**")
}

val buildGoEngine by tasks.registering(Exec::class) {
    group = "build"
    description = "Compile Go torrent engine into libengine.so for arm64-v8a"
    workingDir(repoRoot)
    commandLine("bash", repoRoot.resolve("engine-go/build.sh").absolutePath)
    inputs.files(engineSources)
    outputs.file(file("src/main/jniLibs/arm64-v8a/libengine.so"))
}

// -Pwebtor.emulator=true also packages an x86_64 engine for the emulator.
val emulatorAbi = providers.gradleProperty("webtor.emulator").map { it.toBoolean() }.getOrElse(false)

val buildGoEngineEmulator by tasks.registering(Exec::class) {
    group = "build"
    description = "Compile Go torrent engine into libengine.so for the x86_64 emulator"
    workingDir(repoRoot)
    environment("ENGINE_ABI", "x86_64")
    commandLine("bash", repoRoot.resolve("engine-go/build.sh").absolutePath)
    inputs.files(engineSources)
    outputs.file(file("src/main/jniLibs/x86_64/libengine.so"))
}

tasks.named("preBuild") {
    dependsOn(buildGoEngine)
    if (emulatorAbi) dependsOn(buildGoEngineEmulator)
}

android {
    namespace = "webtor.app"
    compileSdk = 34
    ndkVersion = "26.1.10909125"

    defaultConfig {
        applicationId = "webtor.app"
        minSdk = 26
        targetSdk = 34
        versionCode = 98
        versionName = "2.2.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            abiFilters.add("arm64-v8a")
            if (emulatorAbi) abiFilters.add("x86_64")
        }
    }

    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }

    val releaseStore = providers.environmentVariable("WEBTOR_KEYSTORE").orNull
    signingConfigs {
        if (!releaseStore.isNullOrBlank()) {
            create("production") {
                storeFile = file(releaseStore)
                storePassword = providers.environmentVariable("WEBTOR_STORE_PASSWORD").orNull
                keyAlias = providers.environmentVariable("WEBTOR_KEY_ALIAS").orNull
                keyPassword = providers.environmentVariable("WEBTOR_KEY_PASSWORD").orNull
            }
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
            signingConfig = when {
                !releaseStore.isNullOrBlank() -> signingConfigs.getByName("production")
                providers.gradleProperty("webtor.localSigning").orNull == "true" -> signingConfigs.getByName("debug")
                else -> null
            }
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
        noCompress += "mmdb"
    }

    lint {
        // The release lint pass took 32 s of every build; run ./gradlew lint when wanted.
        checkReleaseBuilds = false
    }
}

buildGoEngine.configure {
    // build.sh stamps the engine with versionName.
    inputs.property("versionName", android.defaultConfig.versionName)
    environment("ANDROID_HOME", android.sdkDirectory.absolutePath)
    environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
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
    implementation("androidx.compose.material:material-icons-core")

    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.activity:activity-compose:1.9.3")
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
