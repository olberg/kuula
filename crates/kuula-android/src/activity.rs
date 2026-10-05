//! What the activity is asked through JNI, on the Java main thread.
//!
//! Finishing is what `sys.exit()` in the shell means. The NDK has
//! `ANativeActivity_finish`, but `android-activity` does not hand out the
//! pointer it needs, so this calls `Activity.finish()`.
//!
//! Hiding the system bars is what a game does: the status bar and the
//! navigation buttons would lie over the picture's edge and beside the
//! D-pad. A swipe from the edge brings them back for a moment.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use android_activity::AndroidApp;
use jni::objects::{JObject, JString, JValue};
use jni::strings::JNIString;
use jni::sys::jobject;
use jni::JavaVM;

/// `View.SYSTEM_UI_FLAG_*`: fullscreen, no navigation, sticky immersive,
/// and the layout laid out as if both bars were gone, so that hiding them
/// does not resize the window.
const IMMERSIVE: i32 = 0x0004 | 0x0002 | 0x1000 | 0x0100 | 0x0200 | 0x0400;

/// Hide the status bar and the navigation buttons. Asked again whenever
/// the activity comes to the front, since a dialog or a turn can bring
/// them back.
pub fn hide_system_bars(app: &AndroidApp) {
    let vm = app.vm_as_ptr() as usize;
    let activity = app.activity_as_ptr() as usize;
    app.run_on_java_main_thread(Box::new(move || {
        // SAFETY: as in `finish` below.
        let vm = unsafe { JavaVM::from_raw(vm as *mut _) };
        let result = vm.attach_current_thread(|env| {
            let activity = unsafe { JObject::from_raw(env, activity as jobject) };
            let window = env
                .call_method(
                    &activity,
                    JNIString::new("getWindow"),
                    jni::jni_sig!("()Landroid/view/Window;"),
                    &[],
                )?
                .l()?;
            let view = env
                .call_method(
                    &window,
                    JNIString::new("getDecorView"),
                    jni::jni_sig!("()Landroid/view/View;"),
                    &[],
                )?
                .l()?;
            env.call_method(
                &view,
                JNIString::new("setSystemUiVisibility"),
                jni::jni_sig!("(I)V"),
                &[JValue::Int(IMMERSIVE)],
            )?;
            Ok::<(), jni::errors::Error>(())
        });
        if let Err(e) = result {
            log::warn!("cannot hide the system bars: {e}");
        }
    }));
}

/// The display cutout's safe insets as the activity last reported them:
/// left, top, right and bottom, 16 bits each. The answer comes from the
/// Java main thread, so it is left here for the loop to read.
#[derive(Clone, Default)]
pub struct Cutout(Arc<AtomicU64>);

impl Cutout {
    /// Left, top, right, bottom, in window pixels; zeros until told.
    pub fn insets(&self) -> [i32; 4] {
        let v = self.0.load(Ordering::Relaxed);
        [0, 16, 32, 48].map(|shift| ((v >> shift) & 0xffff) as i32)
    }

    fn set(&self, insets: [i32; 4]) {
        let mut v = 0u64;
        for (i, inset) in insets.into_iter().enumerate() {
            v |= (inset.clamp(0, 0xffff) as u64) << (16 * i);
        }
        self.0.store(v, Ordering::Relaxed);
    }
}

/// Ask where the display's cutout (a camera hole, a notch) leaves the
/// window clear. The app draws into the whole display, cutout included,
/// and the content rectangle of a native activity does not say where it
/// is. `DisplayCutout` exists from Android 9 (API 28); before it, and on
/// a display without one, the insets are zero.
pub fn ask_cutout(app: &AndroidApp, cutout: &Cutout) {
    if app.config().sdk_version() < 28 {
        return;
    }
    let vm = app.vm_as_ptr() as usize;
    let activity = app.activity_as_ptr() as usize;
    let cutout = cutout.clone();
    app.run_on_java_main_thread(Box::new(move || {
        // SAFETY: as in `finish` below.
        let vm = unsafe { JavaVM::from_raw(vm as *mut _) };
        let result = vm.attach_current_thread(|env| {
            let activity = unsafe { JObject::from_raw(env, activity as jobject) };
            let window = env
                .call_method(
                    &activity,
                    JNIString::new("getWindow"),
                    jni::jni_sig!("()Landroid/view/Window;"),
                    &[],
                )?
                .l()?;
            let view = env
                .call_method(
                    &window,
                    JNIString::new("getDecorView"),
                    jni::jni_sig!("()Landroid/view/View;"),
                    &[],
                )?
                .l()?;
            let insets = env
                .call_method(
                    &view,
                    JNIString::new("getRootWindowInsets"),
                    jni::jni_sig!("()Landroid/view/WindowInsets;"),
                    &[],
                )?
                .l()?;
            // Not attached to a window yet: asked again at the next event.
            if insets.is_null() {
                return Ok(());
            }
            let hole = env
                .call_method(
                    &insets,
                    JNIString::new("getDisplayCutout"),
                    jni::jni_sig!("()Landroid/view/DisplayCutout;"),
                    &[],
                )?
                .l()?;
            if hole.is_null() {
                cutout.set([0; 4]);
                return Ok(());
            }
            let mut safe = [0i32; 4];
            let getters = [
                "getSafeInsetLeft",
                "getSafeInsetTop",
                "getSafeInsetRight",
                "getSafeInsetBottom",
            ];
            for (slot, getter) in safe.iter_mut().zip(getters) {
                *slot = env
                    .call_method(&hole, JNIString::new(getter), jni::jni_sig!("()I"), &[])?
                    .i()?;
            }
            cutout.set(safe);
            Ok::<(), jni::errors::Error>(())
        });
        if let Err(e) = result {
            log::warn!("cannot read the display cutout: {e}");
        }
    }));
}

/// What tells one installed package from another: the APK's path, size
/// and time. Every install gives the APK a new path and time, a reinstall
/// of the same version too, so what was copied out of an earlier package
/// is known to be stale. `None` when the activity does not say.
pub fn package_stamp(app: &AndroidApp) -> Option<String> {
    // SAFETY: as in `launch_cart` below.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let activity = app.activity_as_ptr() as jobject;
    let path = vm.attach_current_thread(|env| {
        let activity = unsafe { JObject::from_raw(env, activity) };
        let path = env
            .call_method(
                &activity,
                JNIString::new("getPackageCodePath"),
                jni::jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        if path.is_null() {
            return Ok(None);
        }
        let path = unsafe { JString::from_raw(env, path.into_raw()) };
        Ok::<_, jni::errors::Error>(Some(path.try_to_string(env)?))
    });
    let path = match path {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(e) => {
            log::warn!("cannot ask where the package is: {e}");
            return None;
        }
    };
    let file = std::fs::metadata(&path).ok()?;
    let time = file
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{path}|{}|{time}", file.len()))
}

/// The cart the activity was started on, when the intent that started it
/// names one: `am start ... --es cart hello.cart`, which is how a cart
/// pushed over `adb` is opened. The name is only ever looked up among the
/// carts the shell lists, so an intent from anywhere can do no more than a
/// person choosing from that list; one that is not a plain file name is
/// not taken at all.
pub fn launch_cart(app: &AndroidApp) -> Option<String> {
    // SAFETY: the pointers are the JavaVM and the activity reference that
    // `AndroidApp` gave, valid while the app is.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let activity = app.activity_as_ptr() as jobject;
    let named = vm.attach_current_thread(|env| {
        let activity = unsafe { JObject::from_raw(env, activity) };
        let intent = env
            .call_method(
                &activity,
                JNIString::new("getIntent"),
                jni::jni_sig!("()Landroid/content/Intent;"),
                &[],
            )?
            .l()?;
        if intent.is_null() {
            return Ok(None);
        }
        let key = env.new_string("cart")?;
        let value = env
            .call_method(
                &intent,
                JNIString::new("getStringExtra"),
                jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/String;"),
                &[JValue::Object(&key)],
            )?
            .l()?;
        if value.is_null() {
            return Ok(None);
        }
        let value = unsafe { JString::from_raw(env, value.into_raw()) };
        Ok::<_, jni::errors::Error>(Some(value.try_to_string(env)?))
    });
    match named {
        Ok(Some(name)) if crate::carts::is_plain_name(&name) => Some(name),
        Ok(Some(_)) => {
            log::warn!("the intent's cart is not a file name; not taken");
            None
        }
        Ok(None) => None,
        Err(e) => {
            log::warn!("cannot read the intent: {e}");
            None
        }
    }
}

pub fn finish(app: &AndroidApp) {
    // Raw pointers are not `Send`; as integers they cross to the main
    // thread, where they are made pointers again. The activity outlives the
    // call: it is what is being asked to end.
    let vm = app.vm_as_ptr() as usize;
    let activity = app.activity_as_ptr() as usize;
    app.run_on_java_main_thread(Box::new(move || {
        // SAFETY: the pointers are the JavaVM and the activity reference
        // that `AndroidApp` gave, valid while the app is.
        let vm = unsafe { JavaVM::from_raw(vm as *mut _) };
        let result = vm.attach_current_thread(|env| {
            let activity = unsafe { JObject::from_raw(env, activity as jobject) };
            env.call_method(
                &activity,
                JNIString::new("finish"),
                jni::jni_sig!("()V"),
                &[],
            )?;
            Ok::<(), jni::errors::Error>(())
        });
        if let Err(e) = result {
            log::error!("cannot finish the activity: {e}");
        }
    }));
}
