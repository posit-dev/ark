use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

use amalthea::fixtures::dummy_frontend::DummyConnection;
use amalthea::fixtures::dummy_frontend::DummyFrontend;

use crate::interface::SessionMode;
use crate::repos::DefaultRepos;

// There can be only one frontend per process. Needs to be in a mutex because
// the frontend wraps zmq sockets which are unsafe to send across threads.
//
// This is using `OnceLock` because it provides a way of checking whether the
// value has been initialized already. Also we'll need to parameterize
// initialization in the future.
static FRONTEND: OnceLock<Arc<Mutex<DummyFrontend>>> = OnceLock::new();

/// Wrapper around `DummyFrontend` that checks sockets are empty on drop
pub struct DummyArkFrontend {
    guard: MutexGuard<'static, DummyFrontend>,
}

struct DummyArkFrontendOptions {
    interactive: bool,
    site_r_profile: bool,
    user_r_profile: bool,
    r_environ: bool,
    session_mode: SessionMode,
    default_repos: DefaultRepos,
    startup_file: Option<String>,
}

/// Wrapper around `DummyArkFrontend` that uses `SessionMode::Notebook`
///
/// Only one of `DummyArkFrontend` or `DummyArkFrontendNotebook` can be used in
/// a given process. Just don't import both and you should be fine as Rust will
/// let you know about a missing symbol if you happen to copy paste `lock()`
/// calls of different kernel types between files.
pub struct DummyArkFrontendNotebook {
    inner: DummyArkFrontend,
}

/// Wrapper around `DummyArkFrontend` that allows an `.Rprofile` to run
pub struct DummyArkFrontendRprofile {
    inner: DummyArkFrontend,
}

/// Wrapper around `DummyArkFrontend` that allows setting default repos
/// for the frontend
pub struct DummyArkFrontendDefaultRepos {
    inner: DummyArkFrontend,
}

impl DummyArkFrontend {
    pub fn lock() -> Self {
        Self {
            guard: Self::get_frontend().lock().unwrap(),
        }
    }

    fn get_frontend() -> &'static Arc<Mutex<DummyFrontend>> {
        // These are the hard-coded defaults. Call `init()` explicitly to
        // override.
        let options = DummyArkFrontendOptions::default();
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))))
    }

    fn init(options: DummyArkFrontendOptions) -> DummyFrontend {
        if FRONTEND.get().is_some() {
            panic!("Can't spawn Ark more than once");
        }

        // We don't want cli to try and restore the cursor, it breaks our tests
        // by adding unecessary ANSI escapes. We don't need this in Positron because
        // cli also checks `isatty(stdout())`, which is false in Positron because
        // we redirect stdout.
        // https://github.com/r-lib/cli/blob/1220ed092c03e167ff0062e9839c81d7258a4600/R/onload.R#L33-L40
        unsafe { std::env::set_var("R_CLI_HIDE_CURSOR", "false") };

        let connection = DummyConnection::new();
        let (connection_file, registration_file) = connection.get_connection_files();

        let mut r_args = vec![];

        // We aren't animals!
        r_args.push(String::from("--no-save"));
        r_args.push(String::from("--no-restore"));

        if options.interactive {
            r_args.push(String::from("--interactive"));
        }
        if !options.site_r_profile {
            r_args.push(String::from("--no-site-file"));
        }
        if !options.user_r_profile {
            r_args.push(String::from("--no-init-file"));
        }
        if !options.r_environ {
            r_args.push(String::from("--no-environ"));
        }

        // Start the kernel and REPL in a background thread, does not return and is never joined.
        // Must run `start_kernel()` in a background thread because it blocks until it receives
        // a `HandshakeReply`, which we send from `from_connection()` below.
        stdext::spawn!("dummy_kernel", move || {
            crate::start::start_kernel(
                connection_file,
                Some(registration_file),
                r_args,
                options.startup_file,
                options.session_mode,
                false,
                options.default_repos,
            );
        });

        DummyFrontend::from_connection(connection)
    }
}

// Check that we haven't left crumbs behind
impl Drop for DummyArkFrontend {
    fn drop(&mut self) {
        self.assert_no_incoming()
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontend {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.guard)
    }
}

impl DerefMut for DummyArkFrontend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.guard)
    }
}

impl DummyArkFrontendNotebook {
    /// Lock a notebook frontend.
    ///
    /// NOTE: Only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with Notebook session mode
    fn init() {
        let mut options = DummyArkFrontendOptions::default();
        options.session_mode = SessionMode::Notebook;
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))));
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendNotebook {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.inner)
    }
}

impl DerefMut for DummyArkFrontendNotebook {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.inner)
    }
}

impl DummyArkFrontendDefaultRepos {
    /// Lock a frontend with a default repos setting.
    ///
    /// NOTE: `startup_file` is required because you typically want
    /// to force `options(repos =)` to a fixed value for testing, regardless
    /// of what the caller's default `repos` are set as (i.e. rig typically
    /// sets it to a non-`@CRAN@` value).
    ///
    /// NOTE: Only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process.
    pub fn lock(default_repos: DefaultRepos, startup_file: String) -> Self {
        Self::init(default_repos, startup_file);

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with given default repos
    fn init(default_repos: DefaultRepos, startup_file: String) {
        let mut options = DummyArkFrontendOptions::default();
        options.default_repos = default_repos;
        options.startup_file = Some(startup_file);

        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))));
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendDefaultRepos {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.inner)
    }
}
impl DummyArkFrontendRprofile {
    /// Lock a frontend that supports `.Rprofile`s.
    ///
    /// NOTE: This variant can only be called exactly once per process,
    /// because you can only load an `.Rprofile` one time. Additionally,
    /// only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process. Practically, this ends up meaning you can only
    /// have 1 test block per integration test that uses a
    /// `DummyArkFrontendRprofile`.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with user level `.Rprofile` enabled
    fn init() {
        let mut options = DummyArkFrontendOptions::default();
        options.user_r_profile = true;
        let status = FRONTEND.set(Arc::new(Mutex::new(DummyArkFrontend::init(options))));

        if status.is_err() {
            panic!("You can only call `DummyArkFrontendRprofile::lock()` once per process.");
        }

        FRONTEND.get().unwrap();
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendRprofile {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.inner)
    }
}

impl DerefMut for DummyArkFrontendRprofile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.inner)
    }
}

impl Default for DummyArkFrontendOptions {
    fn default() -> Self {
        Self {
            interactive: true,
            site_r_profile: false,
            user_r_profile: false,
            r_environ: false,
            session_mode: SessionMode::Console,
            default_repos: DefaultRepos::Auto,
            startup_file: None,
        }
    }
}
