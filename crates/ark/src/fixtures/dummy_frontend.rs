use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

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

        let frontend = DummyFrontend::new();
        let connection_file = frontend.get_connection_file();

        // Start the kernel in this thread so that panics are propagated
        crate::start::start_kernel(
            connection_file,
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

        // Start the REPL in a background thread, does not return and is never joined
        stdext::spawn!("dummy_kernel", || {
            RMain::start();
        });

        frontend.complete_initialization();
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
