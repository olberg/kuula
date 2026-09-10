//! The worker's restricted token.
//!
//! `spawn` starts `<exe> <args>` inside a Less Privileged AppContainer
//! (LPAC) with no capabilities: `CreateProcessW` with
//! `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` (the container SID,
//! zero capability SIDs), `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_
//! PACKAGES_POLICY` set to opt out of the `ALL APPLICATION PACKAGES`
//! grants, and `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` naming exactly three
//! handles: the read end of the child's stdin pipe, the write end of its
//! stdout pipe and a duplicate of the broker's stderr. Nothing else is
//! inherited.
//! The child is created suspended, the caller's `before_resume` hook
//! runs (the broker assigns its Job Object there), and only then is the
//! main thread resumed. Any failure after creation terminates the child
//! and the caller reports `sandbox_unavailable`: the run ends instead of
//! running unsandboxed.
//!
//! The AppContainer profile is named [`PROFILE_NAME`]. It is created on
//! first use with `CreateAppContainerProfile` and its SID derived from
//! the name afterwards, so it is per user and persistent; Windows gives
//! it a redirected `%TEMP%` under `%LOCALAPPDATA%\Packages\<name>\AC`,
//! which the worker never uses because it reads only its pipes. An
//! AppContainer can only run an image its SID may read and execute, and
//! `kuula.exe` lives wherever the user put it, so before every launch
//! the broker adds two non-inherited allow ACEs (read + execute for the
//! container SID and for `ALL RESTRICTED APPLICATION PACKAGES`, which
//! LPAC checks instead of `ALL APPLICATION PACKAGES`) to the executable's
//! DACL. `SetEntriesInAclW` merges an identical ACE, so the grant is
//! idempotent; both ACEs are left in place.
//!

use std::fs::File;

/// The AppContainer profile the worker runs under.
pub const PROFILE_NAME: &str = "kuula-worker";

/// Test hook: when set, setup fails deliberately after the child exists
/// suspended, so the fail-closed path (terminate, report) is exercised.
pub const FAIL_HOOK: &str = "KUULA_TEST_SANDBOX_FAIL";

/// A sandboxed child and the two pipe ends the broker keeps.
pub struct Spawned {
    pub child: Child,
    /// Write end of the child's stdin.
    pub stdin: File,
    /// Read end of the child's stdout.
    pub stdout: File,
}

/// `kuula sandbox-exec -- <args...>`: run this executable with `args`
/// through the same launcher the worker uses, relaying its stdout, and
/// exit with its code. Exists so the denial tests prove the worker's
/// launch path and not a look-alike.
pub fn exec(args: &[String]) -> u8 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("sandbox-exec: cannot locate own executable: {e}");
            return 1;
        }
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let env = minimal_env();
    let mut spawned = match spawn(&exe, &args, &env, |_| Ok(())) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", kuula_core::Fault::SANDBOX_UNAVAILABLE);
            return 1;
        }
    };
    drop(spawned.stdin);
    let mut out = std::io::stdout().lock();
    let _ = std::io::copy(&mut spawned.stdout, &mut out);
    match spawned.child.wait() {
        Ok(code) => (code & 0xff) as u8,
        Err(e) => {
            eprintln!("sandbox-exec: {e}");
            1
        }
    }
}

/// The environment the broker passes to a worker: what the C runtime
/// and loader want, `LOCALAPPDATA` because AppContainer creation derives
/// the container's redirected profile path from it and fails with
/// `ERROR_ENVVAR_NOT_FOUND` (203) without it, plus the worker's test
/// hooks when set.
pub fn minimal_env() -> Vec<(String, String)> {
    [
        "SYSTEMROOT",
        "WINDIR",
        "LOCALAPPDATA",
        crate::worker::CRASH_HOOK,
        crate::worker::HANG_HOOK,
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
    .collect()
}

pub use imp::{spawn, Child};

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fs::File;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, GetLastError, LocalFree, SetHandleInformation,
        DUPLICATE_SAME_ACCESS, ERROR_ALREADY_EXISTS, HANDLE, HANDLE_FLAG_INHERIT,
        INVALID_HANDLE_VALUE, STILL_ACTIVE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW,
        EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID,
        TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
    };
    use windows_sys::Win32::Security::{
        FreeSid, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, PSID, SECURITY_ATTRIBUTES,
        SECURITY_CAPABILITIES,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
        CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    /// Exit code given to a child that is terminated by the broker.
    const KILLED: u32 = 0xC000_013A;

    /// `PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT`: the value of
    /// the policy attribute that makes the container an LPAC.
    const ALL_APPLICATION_PACKAGES_OPT_OUT: u32 = 1;

    /// The well-known `ALL RESTRICTED APPLICATION PACKAGES` group that
    /// an LPAC token carries in place of `ALL APPLICATION PACKAGES`.
    const ALL_RESTRICTED_APPLICATION_PACKAGES: &str = "S-1-15-2-2";

    /// A process handle: kill, wait, poll. Closed on drop; dropping does
    /// not kill (the broker's `Worker` does that first).
    pub struct Child {
        process: HANDLE,
        pid: u32,
    }

    // SAFETY: a process handle is a kernel object reference with no
    // thread affinity.
    unsafe impl Send for Child {}

    impl Child {
        pub fn id(&self) -> u32 {
            self.pid
        }

        pub fn raw_handle(&self) -> std::os::windows::io::RawHandle {
            self.process
        }

        /// TerminateProcess; a process that already exited is fine.
        pub fn kill(&mut self) -> Result<(), String> {
            // SAFETY: valid owned process handle.
            unsafe {
                if TerminateProcess(self.process, KILLED) == 0 {
                    let err = GetLastError();
                    if self.try_wait()?.is_none() {
                        return Err(format!("TerminateProcess failed: {err}"));
                    }
                }
            }
            Ok(())
        }

        /// Block until exit and return the exit code.
        pub fn wait(&mut self) -> Result<u32, String> {
            // SAFETY: valid owned process handle.
            unsafe {
                if WaitForSingleObject(self.process, INFINITE) != WAIT_OBJECT_0 {
                    return Err(format!("WaitForSingleObject failed: {}", GetLastError()));
                }
            }
            self.try_wait().map(|c| c.unwrap_or(0))
        }

        /// The exit code if the process has exited.
        pub fn try_wait(&mut self) -> Result<Option<u32>, String> {
            // SAFETY: valid owned process handle; `code` is written on success.
            unsafe {
                match WaitForSingleObject(self.process, 0) {
                    WAIT_OBJECT_0 => {}
                    WAIT_TIMEOUT => return Ok(None),
                    _ => return Err(format!("WaitForSingleObject failed: {}", GetLastError())),
                }
                let mut code = 0u32;
                if GetExitCodeProcess(self.process, &mut code) == 0 {
                    return Err(format!("GetExitCodeProcess failed: {}", GetLastError()));
                }
                if code == STILL_ACTIVE as u32 {
                    return Ok(None);
                }
                Ok(Some(code))
            }
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            // SAFETY: closed exactly once.
            unsafe {
                CloseHandle(self.process);
            }
        }
    }

    /// A handle closed on drop, for the many temporaries below.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: owned handle, closed once.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    /// A SID and how to release it: `FreeSid` for the container SID that
    /// userenv allocates, `LocalFree` for one parsed from a string.
    struct Sid(PSID, bool);

    impl Drop for Sid {
        fn drop(&mut self) {
            // SAFETY: released exactly once with the matching free.
            unsafe {
                if self.1 {
                    FreeSid(self.0);
                } else {
                    LocalFree(self.0);
                }
            }
        }
    }

    fn string_sid(text: &str) -> Result<Sid, String> {
        let text = wide(text);
        let mut sid: PSID = std::ptr::null_mut();
        // SAFETY: NUL-terminated input; the out-pointer is a local.
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) } == 0 {
            return Err(last_error("ConvertStringSidToSidW"));
        }
        Ok(Sid(sid, false))
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn last_error(what: &str) -> String {
        // SAFETY: plain thread-local read.
        format!("{what} failed: error {}", unsafe { GetLastError() })
    }

    /// Create the profile, or derive its SID when it already exists.
    fn profile_sid() -> Result<Sid, String> {
        let name = wide(super::PROFILE_NAME);
        let display = wide("Kuula worker");
        let description = wide("Kuula cart worker process");
        let mut sid: PSID = std::ptr::null_mut();
        // SAFETY: all pointers are to live, NUL-terminated buffers; zero
        // capabilities so the capability pointer may be null.
        let hr = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                display.as_ptr(),
                description.as_ptr(),
                std::ptr::null(),
                0,
                &mut sid,
            )
        };
        if hr >= 0 {
            return Ok(Sid(sid, true));
        }
        let already = 0x8007_0000u32 | ERROR_ALREADY_EXISTS;
        if hr as u32 != already {
            return Err(format!(
                "CreateAppContainerProfile failed: HRESULT {hr:#010x}"
            ));
        }
        // SAFETY: as above.
        let hr = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
        if hr < 0 {
            return Err(format!(
                "DeriveAppContainerSidFromAppContainerName failed: HRESULT {hr:#010x}"
            ));
        }
        Ok(Sid(sid, true))
    }

    /// Add `read + execute` for `sid` to the executable's DACL.
    /// `SetEntriesInAclW` merges an identical existing ACE, so repeating
    /// this is harmless.
    fn grant_execute(exe: &Path, sid: &Sid) -> Result<(), String> {
        let path = wide(&exe.to_string_lossy());
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid path; the descriptor is freed below with LocalFree
        // as documented; the new ACL likewise.
        unsafe {
            let err = GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut old_dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            );
            if err != 0 {
                return Err(format!(
                    "GetNamedSecurityInfoW({}) failed: error {err}",
                    exe.display()
                ));
            }
            let entry = EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: sid.0 as *mut u16,
                },
            };
            let mut new_dacl: *mut ACL = std::ptr::null_mut();
            let err = SetEntriesInAclW(1, &entry, old_dacl, &mut new_dacl);
            if err != 0 {
                LocalFree(descriptor);
                return Err(format!("SetEntriesInAclW failed: error {err}"));
            }
            let err = SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null(),
            );
            LocalFree(new_dacl as *mut c_void);
            LocalFree(descriptor);
            if err != 0 {
                return Err(format!(
                    "SetNamedSecurityInfoW({}) failed: error {err}",
                    exe.display()
                ));
            }
        }
        Ok(())
    }

    /// An anonymous pipe whose `child` end is inheritable and whose
    /// `ours` end is not.
    fn pipe(child_reads: bool) -> Result<(Owned, Owned), String> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let mut read: HANDLE = std::ptr::null_mut();
        let mut write: HANDLE = std::ptr::null_mut();
        // SAFETY: out-pointers to locals; both handles are owned below.
        unsafe {
            if CreatePipe(&mut read, &mut write, &sa, 0) == 0 {
                return Err(last_error("CreatePipe"));
            }
            let (read, write) = (Owned(read), Owned(write));
            let (ours, child) = if child_reads {
                (write, read)
            } else {
                (read, write)
            };
            if SetHandleInformation(ours.0, HANDLE_FLAG_INHERIT, 0) == 0 {
                return Err(last_error("SetHandleInformation"));
            }
            Ok((ours, child))
        }
    }

    /// An inheritable duplicate of this process's stderr, if it has one.
    fn inheritable_stderr() -> Result<Option<Owned>, String> {
        // SAFETY: GetStdHandle returns a borrowed handle or null/invalid;
        // DuplicateHandle writes the out-pointer on success.
        unsafe {
            let err = GetStdHandle(STD_ERROR_HANDLE);
            if err.is_null() || err == INVALID_HANDLE_VALUE {
                return Ok(None);
            }
            let me = GetCurrentProcess();
            let mut dup: HANDLE = std::ptr::null_mut();
            if DuplicateHandle(me, err, me, &mut dup, 0, 1, DUPLICATE_SAME_ACCESS) == 0 {
                return Err(last_error("DuplicateHandle(stderr)"));
            }
            Ok(Some(Owned(dup)))
        }
    }

    /// Quote one argument the way the MSVC C runtime unquotes it.
    fn quote(arg: &str, out: &mut String) {
        if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
            out.push_str(arg);
            return;
        }
        out.push('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    backslashes = 0;
                    out.push('"');
                }
                _ => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    backslashes = 0;
                    out.push(c);
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
    }

    fn environment_block(env: &[(String, String)]) -> Vec<u16> {
        let mut block: Vec<u16> = Vec::new();
        for (k, v) in env {
            block.extend(std::ffi::OsStr::new(k).encode_wide());
            block.push(u16::from(b'='));
            block.extend(std::ffi::OsStr::new(v).encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    /// The `PROC_THREAD_ATTRIBUTE_LIST` buffer; deleted on drop.
    struct Attributes(Vec<u8>);

    impl Attributes {
        fn ptr(&mut self) -> *mut c_void {
            self.0.as_mut_ptr() as *mut c_void
        }
    }

    impl Drop for Attributes {
        fn drop(&mut self) {
            // SAFETY: initialised by InitializeProcThreadAttributeList.
            unsafe {
                DeleteProcThreadAttributeList(self.ptr());
            }
        }
    }

    /// Start `exe args` in the LPAC, suspended; run `before_resume`;
    /// resume. Every failure after `CreateProcessW` terminates the child
    /// before returning `Err`.
    pub fn spawn(
        exe: &Path,
        args: &[&str],
        env: &[(String, String)],
        before_resume: impl FnOnce(&Child) -> Result<(), String>,
    ) -> Result<super::Spawned, String> {
        let sid = profile_sid()?;
        grant_execute(exe, &sid)?;
        grant_execute(exe, &string_sid(ALL_RESTRICTED_APPLICATION_PACKAGES)?)?;
        let mut policy: u32 = ALL_APPLICATION_PACKAGES_OPT_OUT;
        let (our_stdin, child_stdin) = pipe(true)?;
        let (our_stdout, child_stdout) = pipe(false)?;
        let stderr = inheritable_stderr()?;

        let mut handles: Vec<HANDLE> = vec![child_stdin.0, child_stdout.0];
        if let Some(e) = &stderr {
            handles.push(e.0);
        }
        let mut caps = SECURITY_CAPABILITIES {
            AppContainerSid: sid.0,
            Capabilities: std::ptr::null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };

        let mut command_line = String::new();
        quote(&exe.to_string_lossy(), &mut command_line);
        for a in args {
            command_line.push(' ');
            quote(a, &mut command_line);
        }
        let mut command_line = wide(&command_line);
        let application = wide(&exe.to_string_lossy());
        let mut block = environment_block(env);

        // SAFETY: every pointer handed to Win32 outlives the call; the
        // attribute list is sized by the API and deleted on drop; the
        // process and thread handles are owned by `Child` and `Owned`.
        let (child, thread) = unsafe {
            let mut size = 0usize;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 3, 0, &mut size);
            let mut buffer = vec![0u8; size];
            if InitializeProcThreadAttributeList(
                buffer.as_mut_ptr() as *mut c_void,
                3,
                0,
                &mut size,
            ) == 0
            {
                return Err(last_error("InitializeProcThreadAttributeList"));
            }
            // Only an initialised list may be deleted, so the owner is
            // built after the call succeeded.
            let mut attrs = Attributes(buffer);
            if UpdateProcThreadAttribute(
                attrs.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &mut caps as *mut _ as *const c_void,
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                return Err(last_error(
                    "UpdateProcThreadAttribute(security capabilities)",
                ));
            }
            if UpdateProcThreadAttribute(
                attrs.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr() as *const c_void,
                handles.len() * std::mem::size_of::<HANDLE>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                return Err(last_error("UpdateProcThreadAttribute(handle list)"));
            }
            if UpdateProcThreadAttribute(
                attrs.ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
                &mut policy as *mut _ as *const c_void,
                std::mem::size_of::<u32>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                return Err(last_error(
                    "UpdateProcThreadAttribute(all application packages policy)",
                ));
            }

            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = child_stdin.0;
            si.StartupInfo.hStdOutput = child_stdout.0;
            si.StartupInfo.hStdError = stderr.as_ref().map_or(std::ptr::null_mut(), |e| e.0);
            si.lpAttributeList = attrs.ptr();
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                block.as_mut_ptr() as *const c_void,
                std::ptr::null(),
                &si.StartupInfo,
                &mut pi,
            );
            if ok == 0 {
                return Err(last_error(&format!("CreateProcessW({})", exe.display())));
            }
            (
                Child {
                    process: pi.hProcess,
                    pid: pi.dwProcessId,
                },
                Owned(pi.hThread),
            )
        };
        // The child owns its copies now.
        drop((child_stdin, child_stdout, stderr));

        let mut child = child;
        let setup = if std::env::var_os(super::FAIL_HOOK).is_some() {
            Err(format!("{} is set", super::FAIL_HOOK))
        } else {
            before_resume(&child)
        };
        if let Err(reason) = setup {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "worker {} terminated before it ran: {reason}",
                child.id()
            ));
        }
        // SAFETY: the thread handle is valid and suspended exactly once.
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            let reason = last_error("ResumeThread");
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "worker {} terminated before it ran: {reason}",
                child.id()
            ));
        }
        drop(thread);

        // SAFETY: both handles are open, owned, and given up here.
        let (stdin, stdout) = unsafe {
            let stdin = File::from_raw_handle(our_stdin.0);
            let stdout = File::from_raw_handle(our_stdout.0);
            std::mem::forget(our_stdin);
            std::mem::forget(our_stdout);
            (stdin, stdout)
        };
        Ok(super::Spawned {
            child,
            stdin,
            stdout,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    /// There is no AppContainer off Windows; `spawn` always fails and
    /// the broker reports `sandbox_unavailable`.
    pub struct Child(());

    impl Child {
        pub fn id(&self) -> u32 {
            0
        }
        pub fn kill(&mut self) -> Result<(), String> {
            Ok(())
        }
        pub fn wait(&mut self) -> Result<u32, String> {
            Ok(0)
        }
        pub fn try_wait(&mut self) -> Result<Option<u32>, String> {
            Ok(Some(0))
        }
    }

    pub fn spawn(
        _exe: &Path,
        _args: &[&str],
        _env: &[(String, String)],
        _before_resume: impl FnOnce(&Child) -> Result<(), String>,
    ) -> Result<super::Spawned, String> {
        Err("AppContainer tokens exist only on Windows".to_string())
    }
}
