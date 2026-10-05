// The package of the Android app: a manifest, resources, the native library
// and the carts. No code is compiled here; the library is built by cargo
// (see Dockerfile) and put under app/src/main/jniLibs before Gradle runs.

pluginManagement {
    repositories {
        google()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "kuula-android"
include(":app")
