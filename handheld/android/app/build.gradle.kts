// The app module: no sources. The native library, the carts and the icon
// are put into src/main/jniLibs, assets and res/mipmap-nodpi by the build
// before this runs, and are not tracked.

plugins {
    id("com.android.application")
}

// From the workspace version in the root Cargo.toml, read by the build
// script: this project does not see that file.
val kuulaVersionName = providers.gradleProperty("kuula.versionName").get()
val kuulaVersionCode = providers.gradleProperty("kuula.versionCode").get().toInt()
// Where the debug keystore lives. It persists between builds so that a new
// build installs over the old one.
val kuulaKeystore = providers.gradleProperty("kuula.keystore").get()

android {
    namespace = "io.github.olberg.kuula"
    // 35, not the newest: a target of 36 changes how the window and its
    // orientation are treated on large screens, which is not looked into yet.
    compileSdk = 35
    // The plugin's default and its minimum.
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "io.github.olberg.kuula"
        // AAudio needs API 26.
        minSdk = 26
        targetSdk = 35
        versionCode = kuulaVersionCode
        versionName = kuulaVersionName
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    signingConfigs {
        // The standard Android debug credentials: the key is not a secret.
        getByName("debug") {
            storeFile = file(kuulaKeystore)
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }

    packaging {
        jniLibs {
            // Libraries stored uncompressed and aligned in the zip, mapped
            // straight from the APK: what 16 KB page devices need.
            useLegacyPackaging = false
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}
