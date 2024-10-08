use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

use amalthea::fixtures::dummy_frontend::DummyConnection;
use amalthea::fixtures::dummy_frontend::DummyFrontend;

use crate::interface::RMain;
use crate::interface::SessionMode;

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

/// Wrapper around `DummyArkFrontend` that uses `SessionMode::Notebook`
///
/// Only one of `DummyArkFrontend` or `DummyArkFrontendNotebook` can be used in
/// a given process. Just don't import both and you should be fine as Rust will
/// let you know about a missing symbol if you happen to copy paste `lock()`
/// calls of different kernel types between files.
pub struct DummyArkFrontendNotebook {
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
        let session_mode = SessionMode::Console;
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(session_mode))))
    }

    pub(crate) fn init(session_mode: SessionMode) -> DummyFrontend {
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

        // Start the kernel and REPL in a background thread, does not return and is never joined.
        // Must run `start_kernel()` in a background thread because it blocks until it receives
        // a `HandshakeReply`, which we send from `from_connection()` below.
        stdext::spawn!("dummy_kernel", move || {
            crate::start::start_kernel(
                connection_file,
                Some(registration_file),
                vec![
                    String::from("--interactive"),
                    String::from("--vanilla"),
                    String::from("--no-save"),
                    String::from("--no-restore"),
                ],
                None,
                session_mode,
                false,
            );

            RMain::start();
        });

        let frontend = DummyFrontend::from_connection(connection);

        frontend
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
    /// NOTE: Only one of `DummyArkFrontendNotebook::lock()` re
    /// `DummyArkFrontend::lock()` should be called in a given process.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with Notebook session mode
    fn init() {
        let session_mode = SessionMode::Notebook;
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(session_mode))));
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
