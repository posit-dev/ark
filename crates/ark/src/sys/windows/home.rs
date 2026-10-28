/*
 * home.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::env;
use std::ffi::CStr;
use std::ffi::CString;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;

use libr::get_R_HOME;
use winsafe::co;
use winsafe::SHGetKnownFolderPath;
use winsafe::WideCharToMultiByte;

/// Find the R home directory to pass to R as `Rstart.rhome`
///
/// `get_R_HOME()` returns UTF-8 in all of its paths due to Ark setting UTF-8 as
/// the active code page (ACP) in the application manifest, so we can use it
/// directly, like RStudio!
/// https://github.com/rstudio/rstudio/blob/d9c0b090d49752fe60e7a2ea4be3123cc3feeb6c/src/cpp/r/session/RDiscovery.cpp#L42
///
/// Here are the paths `get_R_HOME()` can take:
/// - `getenv()` converts from UTF-16 to the ACP via UCRT
/// - `GetEnvironmentVariable()` resolves to `GetEnvironmentVariableA()` in R,
///   which converts from UTF-16 to the ACP
/// - Registry lookup goes through `RegQueryValueEx()`, which resolves to
///   `RegQueryValueExA()` in R, which converts from UTF-16 to the ACP
///
/// https://github.com/wch/r-source/blob/6561734306302f8f22ce2134c47b13a4b96d24f0/src/gnuwin32/rhome.c#L152
/// https://github.com/posit-dev/positron/issues/12303
pub(crate) fn r_home() -> CString {
    let path = unsafe { get_R_HOME() };

    if path.is_null() {
        panic!("`get_R_HOME()` failed to report an R home.");
    }

    unsafe { CStr::from_ptr(path) }.to_owned()
}

/// Find the user home directory to pass to R as `Rstart.home`
///
/// This is a replacement for R's `getRUser()`, because up until R 4.7.0, it
/// didn't return UTF-8 in all cases even though we've set the active code page
/// (ACP) to UTF-8 in the application manifest. RStudio also uses their own
/// custom version of this, presumably for the same reason.
/// https://github.com/rstudio/rstudio/blob/d9c0b090d49752fe60e7a2ea4be3123cc3feeb6c/src/cpp/shared_core/system/Win32User.cpp#L164
///
/// In particular, `getRUser()` used to convert the `Documents` folder path with
/// `wcstombs()`, which didn't go through the ACP. Instead, it used the CRT
/// locale, which is still "C" at the point when ark needs to call it, so
/// non-ASCII paths would come back as Latin-1 or fail entirely. As of R 4.7.0,
/// it now correctly goes through `WideCharToMultiByte(CP_ACP)`, but we have to
/// support older R versions!
/// https://github.com/wch/r-source/blob/6561734306302f8f22ce2134c47b13a4b96d24f0/src/gnuwin32/shext.c#L28-L46
///
/// Our version applies the patch that goes through
/// `WideCharToMultiByte(CP_ACP)`.
pub(crate) fn user_home() -> CString {
    let home = find_user_home(|name| env::var_os(name), documents_dir, current_dir);

    let Some(home) = home else {
        panic!("Can't determine the user home directory.");
    };

    to_native(&home)
}

/// We mirror R's lookup here:
/// - `R_USER`
/// - `HOME`
/// - `Documents` directory
/// - `HOMEDRIVE` + `HOMEPATH` (both must exist)
/// - Current directory
///
/// https://github.com/wch/r-source/blob/6561734306302f8f22ce2134c47b13a4b96d24f0/src/gnuwin32/shext.c#L55-L91
fn find_user_home(
    env_var: impl Fn(&str) -> Option<OsString>,
    documents_dir: impl FnOnce() -> Option<OsString>,
    current_dir: impl FnOnce() -> Option<OsString>,
) -> Option<OsString> {
    let home = env_var("R_USER")
        .or_else(|| env_var("HOME"))
        .or_else(documents_dir)
        .or_else(|| {
            let mut home = env_var("HOMEDRIVE")?;
            home.push(env_var("HOMEPATH")?);
            Some(home)
        })
        .or_else(current_dir)?;

    Some(strip_trailing_separators(&home))
}

fn documents_dir() -> Option<OsString> {
    // Same flag as R's `ShellGetPersonalDirectory()`
    let dir = SHGetKnownFolderPath(&co::KNOWNFOLDERID::Documents, co::KF::CREATE, None).ok()?;
    Some(OsString::from(dir))
}

fn current_dir() -> Option<OsString> {
    // Goes through `GetCurrentDirectory()` like R
    let dir = env::current_dir().ok()?;
    Some(dir.into_os_string())
}

/// Remove trailing separators, except the one of a drive root like `C:\`
/// https://github.com/wch/r-source/blob/6561734306302f8f22ce2134c47b13a4b96d24f0/src/gnuwin32/shext.c#L87-L89
fn strip_trailing_separators(path: &OsStr) -> OsString {
    let mut path: Vec<u16> = path.encode_wide().collect();

    while has_strippable_separator(&path) {
        path.pop();
    }

    OsString::from_wide(&path)
}

fn has_strippable_separator(path: &[u16]) -> bool {
    // R never strips the first character
    let [.., before, last] = path else {
        return false;
    };

    if *last != u16::from(b'/') && *last != u16::from(b'\\') {
        return false;
    }

    path.len() > 3 || *before != u16::from(b':')
}

fn to_native(path: &OsStr) -> CString {
    let path: Vec<u16> = path.encode_wide().collect();

    // `WideCharToMultiByte()` reports an error for empty input
    if path.is_empty() {
        return CString::default();
    }

    // Characters that aren't representable in the ANSI code page are replaced
    // with the default character, as in R
    let bytes = match WideCharToMultiByte(co::CP::ACP, co::WC::NoValue, &path, None, None) {
        Ok(bytes) => bytes,
        Err(err) => panic!("Can't convert the user home directory to the native encoding: {err}"),
    };

    match CString::new(bytes) {
        Ok(path) => path,
        Err(err) => panic!("User home directory contains a nul byte: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    fn some(path: &str) -> impl FnOnce() -> Option<OsString> + '_ {
        move || Some(OsString::from(path))
    }

    fn none() -> Option<OsString> {
        None
    }

    fn not_called() -> Option<OsString> {
        panic!("Lookup should not be reached");
    }

    #[test]
    fn test_r_user_wins() {
        let env = env_from(&[
            ("R_USER", r"C:\r_user"),
            ("HOME", r"C:\home"),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\homepath"),
        ]);
        let home = find_user_home(env, not_called, not_called);
        assert_eq!(home, Some(OsString::from(r"C:\r_user")));
    }

    #[test]
    fn test_empty_r_user_still_wins() {
        let env = env_from(&[("R_USER", ""), ("HOME", r"C:\home")]);
        let home = find_user_home(env, not_called, not_called);
        assert_eq!(home, Some(OsString::from("")));
    }

    #[test]
    fn test_home_after_r_user() {
        let env = env_from(&[
            ("HOME", r"C:\home"),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\homepath"),
        ]);
        let home = find_user_home(env, not_called, not_called);
        assert_eq!(home, Some(OsString::from(r"C:\home")));
    }

    #[test]
    fn test_documents_after_home() {
        let env = env_from(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\homepath")]);
        let home = find_user_home(env, some(r"C:\Users\Strøm\Documents"), not_called);
        assert_eq!(home, Some(OsString::from(r"C:\Users\Strøm\Documents")));
    }

    #[test]
    fn test_homedrive_homepath_after_documents() {
        let env = env_from(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\homepath")]);
        let home = find_user_home(env, none, not_called);
        assert_eq!(home, Some(OsString::from(r"C:\homepath")));
    }

    #[test]
    fn test_homedrive_without_homepath_is_skipped() {
        let env = env_from(&[("HOMEDRIVE", "C:")]);
        let home = find_user_home(env, none, some(r"C:\cwd"));
        assert_eq!(home, Some(OsString::from(r"C:\cwd")));
    }

    #[test]
    fn test_current_dir_last() {
        let env = env_from(&[]);
        let home = find_user_home(env, none, some(r"C:\cwd"));
        assert_eq!(home, Some(OsString::from(r"C:\cwd")));
    }

    #[test]
    fn test_no_home_found() {
        let env = env_from(&[]);
        let home = find_user_home(env, none, none);
        assert_eq!(home, None);
    }

    #[test]
    fn test_result_is_stripped() {
        let env = env_from(&[("R_USER", r"C:\r_user\")]);
        let home = find_user_home(env, not_called, not_called);
        assert_eq!(home, Some(OsString::from(r"C:\r_user")));
    }

    #[test]
    fn test_strip_trailing_separators() {
        let strip = |path: &str| strip_trailing_separators(OsStr::new(path));

        assert_eq!(strip(r"C:\Users\foo\"), OsString::from(r"C:\Users\foo"));
        assert_eq!(strip(r"C:\Users\foo/\"), OsString::from(r"C:\Users\foo"));
        assert_eq!(strip(r"C:\Users\foo"), OsString::from(r"C:\Users\foo"));
        assert_eq!(strip(r"\\server\share\"), OsString::from(r"\\server\share"));
        assert_eq!(strip(r"C:\"), OsString::from(r"C:\"));
        assert_eq!(strip("C:/"), OsString::from("C:/"));
        assert_eq!(strip(r"C:\\"), OsString::from(r"C:\"));
        assert_eq!(strip(r"\\"), OsString::from(r"\"));
        assert_eq!(strip(r"\"), OsString::from(r"\"));
        assert_eq!(strip(""), OsString::from(""));
    }

    #[test]
    fn test_to_native_non_ascii() {
        // Relies on "native" `co::CP::ACP` being UTF-8 via our manifest
        assert_eq!(
            to_native(OsStr::new(r"C:\Strøm")).as_bytes(),
            b"C:\\Str\xC3\xB8m"
        );
        assert_eq!(
            to_native(OsStr::new(r"C:\中")).as_bytes(),
            b"C:\\\xE4\xB8\xAD"
        );
    }

    #[test]
    fn test_to_native() {
        assert_eq!(
            to_native(OsStr::new(r"C:\Users\foo")),
            CString::from(c"C:\\Users\\foo")
        );
        assert_eq!(to_native(OsStr::new("")), CString::default());
    }

    #[test]
    fn test_user_home_from_r_user() {
        // Ensures non-ASCII `R_USER` passes through as the right UTF-8 bytes
        unsafe { env::set_var("R_USER", r"C:\Users\Strøm\") };
        assert_eq!(user_home().as_bytes(), b"C:\\Users\\Str\xC3\xB8m");
    }
}
