# syntax=docker/dockerfile:1
# check=skip=FromPlatformFlagConstDisallowed
#
# The Android build: the native library for three ABIs, and the tools that
# pack and check the APK (Gradle, the SDK's build-tools). Everything is in
# the image, so nothing is installed on the build machine. By hand, from the
# repository root:
#
#   docker build -f handheld/android/Dockerfile --target artifacts \
#       --output type=local,dest=dist/android-build/libs .
#
# Versions, and why:
#   Ubuntu 24.04 with OpenJDK 21: the Android Gradle plugin needs 17 or newer.
#   Android Gradle plugin 9.4.1 (handheld/android/build.gradle.kts) wants
#     Gradle 9.6 or newer: 9.6.1, its default.
#   Platform android-35 (compileSdk and targetSdk) and build-tools 36.0.0,
#     which the plugin requires as its minimum.
#   NDK r28c (28.2.13676358): r28 links for 16 KB pages by default, and it is
#     the plugin's default NDK.
#   Rust 1.98.1 as in handheld/miyoo/Dockerfile, cargo-ndk 4.1.2.

ARG RUST_VERSION=1.98.1

# The NDK ships Linux x86-64 toolchains only.
FROM --platform=linux/amd64 ubuntu:24.04 AS sdk
ENV DEBIAN_FRONTEND=noninteractive
# build-essential: the host half of a build script (cc, ld) is not the NDK's.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl unzip zip openjdk-21-jdk-headless build-essential \
    && rm -rf /var/lib/apt/lists/*
ENV ANDROID_HOME=/opt/android-sdk \
    ANDROID_NDK_HOME=/opt/android-sdk/ndk/28.2.13676358
# Command-line tools 23.0 (the file's SHA-1 is from Google's repository index).
RUN set -e; \
    curl -fsSL -o /tmp/cmdline.zip https://dl.google.com/android/repository/commandlinetools-linux-16111833_latest.zip; \
    echo "e025545c62a8e64c7559119566a569fb1dec5f60  /tmp/cmdline.zip" | sha1sum -c -; \
    mkdir -p $ANDROID_HOME/cmdline-tools; \
    unzip -q /tmp/cmdline.zip -d $ANDROID_HOME/cmdline-tools; \
    mv $ANDROID_HOME/cmdline-tools/cmdline-tools $ANDROID_HOME/cmdline-tools/latest; \
    rm /tmp/cmdline.zip
ENV PATH=$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/build-tools/36.0.0:$PATH
# `yes` answers the licence prompts the install raises.
RUN yes | sdkmanager --sdk_root=$ANDROID_HOME \
        "platforms;android-35" "build-tools;36.0.0" "ndk;28.2.13676358" > /tmp/sdkmanager.log \
    || { tail -n 20 /tmp/sdkmanager.log; exit 1; }; \
    test -d $ANDROID_NDK_HOME && test -x $ANDROID_HOME/build-tools/36.0.0/aapt2

# ---- Gradle, and what the plugin fetches, so a rebuild does not ----------------
FROM sdk AS gradle
ARG GRADLE_VERSION=9.6.1
ARG GRADLE_SHA256=9c0f7faeeb306cb14e4279a3e084ca6b596894089a0638e68a07c945a32c9e14
RUN set -e; \
    curl -fsSL -o /tmp/gradle.zip https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip; \
    echo "${GRADLE_SHA256}  /tmp/gradle.zip" | sha256sum -c -; \
    unzip -q /tmp/gradle.zip -d /opt; \
    ln -s /opt/gradle-${GRADLE_VERSION} /opt/gradle; \
    rm /tmp/gradle.zip
ENV PATH=/opt/gradle/bin:$PATH \
    GRADLE_USER_HOME=/opt/gradle-home
# A build of the project as it is, with a stand-in icon and keystore, fills
# Gradle's cache with the plugin and the tools it runs. Only the Gradle files
# and the manifest are copied, so a change to other things leaves this layer.
COPY handheld/android/settings.gradle.kts handheld/android/build.gradle.kts handheld/android/gradle.properties /warm/
COPY handheld/android/app/build.gradle.kts /warm/app/
COPY handheld/android/app/src/main/AndroidManifest.xml /warm/app/src/main/
COPY handheld/android/app/src/main/res /warm/app/src/main/res
COPY assets/branding/kuula-app-256.png /warm/app/src/main/res/mipmap-nodpi/ic_launcher.png
RUN set -e; \
    keytool -genkeypair -keystore /tmp/warm.keystore -storepass android -alias androiddebugkey \
        -keypass android -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=warm" > /dev/null 2>&1; \
    cd /warm; \
    gradle --no-daemon --console=plain -q -Pkuula.versionName=0.0.0 -Pkuula.versionCode=1 \
        -Pkuula.keystore=/tmp/warm.keystore :app:assembleDebug; \
    cd /; rm -rf /warm /tmp/warm.keystore

# ---- Rust for the three ABIs and cargo-ndk -------------------------------------
FROM gradle AS toolchain
ARG RUST_VERSION
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain ${RUST_VERSION} --component clippy \
        --target aarch64-linux-android --target armv7-linux-androideabi --target x86_64-linux-android
RUN cargo install cargo-ndk --version 4.1.2 --locked && rm -rf /usr/local/cargo/registry
WORKDIR /src

# ---- the libraries -----------------------------------------------------------
FROM toolchain AS build
COPY . .
ENV CARGO_TARGET_DIR=/cache/target
# cargo-ndk sets CFLAGS_<target> itself (the clang target), with hyphens in
# the name, and `cc` prefers that name over the underscored one that
# .cargo/config.toml sets, so Lua would be built without the fixed hash seed
# and the numeric-profile header. cargo-ndk keeps what it finds under the
# underscored name and puts its own flag in front, so the values are read
# from the config here and exported: one source, and no build without them.
# The file has either line ending (a checkout on Windows has CRLF), so the
# carriage returns go before the lines are matched, not after.
RUN --mount=type=cache,id=kuula-android-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=kuula-android-target,target=/cache/target \
    set -e; \
    for t in aarch64_linux_android armv7_linux_androideabi x86_64_linux_android; do \
        v=$(tr -d '\r' < .cargo/config.toml | sed -n "s/^CFLAGS_${t} = \"\(.*\)\"\$/\1/p" | sed 's/\\"/"/g'); \
        case "$v" in \
            *luai_makeseed*LUA_USER_H*ffp-contract=off*) ;; \
            *) echo "CFLAGS_${t} in .cargo/config.toml lacks the Lua flags: '$v'" >&2; exit 1 ;; \
        esac; \
        export "CFLAGS_${t}=${v}"; \
    done; \
    cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 --platform 26 -o /cache/jniLibs \
        build -p kuula-android --release --locked; \
    NDK_BIN=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin; \
    for abi in arm64-v8a armeabi-v7a x86_64; do \
        mkdir -p /out/jniLibs/$abi /out/unstripped/$abi; \
        cp /cache/jniLibs/$abi/libkuula_android.so /out/unstripped/$abi/; \
        $NDK_BIN/llvm-strip --strip-unneeded -o /out/jniLibs/$abi/libkuula_android.so \
            /cache/jniLibs/$abi/libkuula_android.so; \
    done

FROM scratch AS artifacts
COPY --from=build /out /
