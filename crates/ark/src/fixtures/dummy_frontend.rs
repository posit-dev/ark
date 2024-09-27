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

impl DummyArkFrontend {
    pub fn lock() -> Self {
        Self {
            guard: Self::get_frontend().lock().unwrap(),
        }
    }

    fn get_frontend() -> &'static Arc<Mutex<DummyFrontend>> {
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init())))
    }

    fn init() -> DummyFrontend {
        if FRONTEND.get().is_some() {
            panic!("Can't spawn Ark more than once");
        }

        let frontend = DummyFrontend::new();
        let connection_file = frontend.get_connection_file();

        stdext::spawn!("dummy_kernel", || {
            crate::start::start_kernel(
                connection_file,
                vec![String::from("--no-save"), String::from("--no-restore")],
                None,
                SessionMode::Console,
                false,
            );
        });

        // Wait for startup to complete
        RMain::wait_r_initialized();

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
