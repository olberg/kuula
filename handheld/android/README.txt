Kuula for Android
=================

Kuula's shell with some carts built in, as an app that is sideloaded. It
is played with on-screen controls, with a controller, or with the buttons
of an Android handheld. Android 8.0 or newer; arm64-v8a, armeabi-v7a and
x86_64. The app asks for no permissions and has no networking yet.

Install
  adb install -r Kuula-<version>-debug.apk
  or copy the APK to the device and open it there; Android asks whether
  to allow installing from that source.

Play
  Held sideways, the picture is in the middle with the D-pad on its left
  and A, B and Menu on its right. Held upright, the picture is at the top
  and the controls are below it. The app turns with the device.
  A cart that uses more buttons shows them while it plays: X and Y with
  A and B, L1, L2, R1 and R2 above, Select and Start below.
  Menu opens the pause menu in a cart: resume, restart, settings, quit.
  The system's Back is Menu too.

Buttons of a controller or a handheld
  D-pad                 the arrows
  A, B, X, Y            the same
  L1, R1, L2, R2        the same
  Start, Select         the same; held together they are Menu, and
                        so is either alone in a cart with two buttons;
                        in the lists Start chooses, as A does
  Mode, Back            Menu
  The on-screen controls hide when a controller's button is pressed and
  come back at the next touch.

Your own carts
  A cart is one .cart file: `kuula build <cart directory> --out name.cart`.
  The app lists what is in
      Android/data/io.github.olberg.kuula/files/carts
  on the device's storage, beside its built-in carts, the next time it
  starts. A cart there with the file name of a built-in one takes its
  place. From a computer with adb, one command packs a cart directory,
  puts it there and starts the app on it:
      kuula deploy push <cart directory> --to adb

Build
  Everything is built in Docker, from the repository's root. The Gradle
  project in this directory compiles nothing: it packs and signs a native
  library, the carts and an icon, none of which are in the tree.

  1. The image with the Android SDK, the NDK, Gradle and Rust:
       docker build -f handheld/android/Dockerfile --target toolchain \
           -t kuula-android-toolchain .
  2. The native library for the three ABIs, into
     dist/android-build/libs/jniLibs/<abi>/libkuula_android.so:
       docker build -f handheld/android/Dockerfile --target artifacts \
           --output type=local,dest=dist/android-build/libs .
  3. The project's inputs, under handheld/android/app/src/main/:
       jniLibs/<abi>/libkuula_android.so     from step 2, each ABI
       assets/carts/<name>.cart              each cart to build in, packed
                                             with `kuula build`
       res/mipmap-nodpi/ic_launcher.png      assets/branding/kuula-app-256.png
  4. A key to sign with, once, kept so that a new build installs over the
     old one (these are Android's standard debug credentials):
       keytool -genkeypair -keystore dist/android/debug.keystore \
           -storepass android -alias androiddebugkey -keypass android \
           -keyalg RSA -keysize 2048 -validity 10000 \
           -dname "CN=Android Debug,O=Android,C=US"
     (keytool is in the image, if not on the machine.)
  5. Gradle, in the image; the version is the workspace's, and the version
     code is major * 1000000 + minor * 1000 + patch:
       docker run --rm -v "$PWD/handheld/android:/project" \
           -v "$PWD/dist/android:/out" kuula-android-toolchain bash -c \
           'cd /project && gradle --no-daemon \
                -Pkuula.versionName=<version> -Pkuula.versionCode=<code> \
                -Pkuula.keystore=/out/debug.keystore :app:assembleDebug'
     The APK is handheld/android/app/build/outputs/apk/debug/app-debug.apk.
