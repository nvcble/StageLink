plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin
    // Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "nz.co.stagelink.stagelink_mobile"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "nz.co.stagelink.stagelink_mobile"

        // 29, not Flutter's default.
        //
        // `WIFI_MODE_FULL_LOW_LATENCY` arrives at 29 and is the single most
        // valuable Android API for this product: without it, Wi-Fi power save
        // buffers unicast packets at the access point and delivers them in
        // bursts well over 100 ms, which no sane jitter buffer absorbs.
        // AAudio's low-latency path is also dependable from 29 onwards.
        minSdk = 29
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName

        ndk {
            // Two ABIs. `armeabi-v7a` is irrelevant at minSdk 29, and 32-bit
            // x86 is an emulator-only target at a bitness nothing ships.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }

        externalNativeBuild {
            cmake {
                // Shared STL: both this library and Oboe use it, and two
                // static copies in one process is a known way to get
                // undefined behaviour at a library boundary.
                arguments += listOf("-DANDROID_STL=c++_shared")
                cppFlags += "-O2"

                // Needed as well as the `ndk` block above, which only governs
                // packaging. Without it CMake configures for all four ABIs —
                // including `armeabi-v7a`, which has no cross-compiled Rust
                // library and so fails the guard in CMakeLists.txt.
                abiFilters += listOf("arm64-v8a", "x86_64")
            }
        }
    }

    // Oboe ships as a prefab AAR.
    buildFeatures {
        prefab = true
    }

    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.22.1"
        }
    }

    sourceSets {
        getByName("main") {
            // Where `tool/build_android.ps1` puts the cross-compiled Rust
            // libraries. Gradle does not know how to drive Cargo, and
            // teaching it would couple two build systems for no gain.
            jniLibs.srcDir("src/main/jniLibs")
        }
    }

    buildTypes {
        release {
            // TODO: a real signing config before this is distributed.
            signingConfig = signingConfigs.getByName("debug")

            // Both off, and they have to agree — Gradle refuses to shrink
            // resources without shrinking code.
            //
            // Off rather than on: the app is small, so there is little to
            // gain, and R8 stripping something the JNI layer looks up by name
            // would fail at runtime on a device rather than at build time
            // here. Worth revisiting with a `proguard-rules.pro` that keeps
            // the native surface, but not before the app runs on hardware.
            isMinifyEnabled = false
            isShrinkResources = false
        }
    }

    packaging {
        jniLibs {
            // Keep the symbols: a native crash report with no symbols is
            // worth very little, and the library is 2 MB either way.
            keepDebugSymbols += listOf("**/libstagelink_receiver.so")
            useLegacyPackaging = false

            // Drop armeabi-v7a entirely.
            //
            // Not cosmetic. Plugins pull in 32-bit ARM libraries even when
            // Flutter's own are excluded, and Android decides an APK supports
            // an ABI by looking at which libraries are present — so an armv7
            // phone would happily install this and then crash on
            // `System.loadLibrary("stagelink_receiver")`, because the
            // receiver is only built for arm64 and x86_64. Removing them
            // makes the APK honestly unsupported there, which is a refused
            // install instead of a crash in somebody's hand.
            //
            // minSdk 29 means a 32-bit-only device is ancient anyway.
            excludes += listOf("lib/armeabi-v7a/**", "lib/x86/**")
        }
    }
}

dependencies {
    // Oboe rather than raw AAudio: it handles the device quirks and the
    // pre-AAudio fallback that would otherwise be ours to get wrong on
    // hardware we do not have.
    implementation("com.google.oboe:oboe:1.9.3")
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
