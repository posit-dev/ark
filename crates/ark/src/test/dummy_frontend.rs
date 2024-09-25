use std::ops::Deref;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use amalthea::test::dummy_frontend::DummyFrontend;
use once_cell::sync::Lazy;

use crate::interface::RMain;
use crate::interface::SessionMode;

// There can be only one frontend per process. Needs to be in a mutex because
// the frontend wraps zmq sockets which are unsafe to send across threads.
static FRONTEND: Lazy<Arc<Mutex<DummyFrontend>>> =
    Lazy::new(|| Arc::new(Mutex::new(DummyArkFrontend::init())));

/// Wrapper around `DummyFrontend` that checks sockets are empty on drop
pub struct DummyArkFrontend {
    guard: MutexGuard<'static, DummyFrontend>,
}

impl DummyArkFrontend {
    pub fn lock() -> Self {
        Self {
            guard: FRONTEND.lock().unwrap(),
        }
    }

    fn init() -> DummyFrontend {
        if Lazy::get(&FRONTEND).is_some() {
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

        frontend.complete_intialization();
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
