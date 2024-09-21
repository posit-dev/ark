//
// test.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Wrapper around `harp::r_test_impl()` that also initializes the ark level R
// modules, so they can be utilized in the tests

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::Once;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::socket;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tree_sitter::Point;

use crate::modules;

// Lock for tests that can't be run concurrently. Only needed for tests that use
// `r_test_init()` instead of `r_test()` since the latter takes the R lock.
static TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn r_test<F: FnOnce()>(f: F) {
    let f = || {
        initialize_ark();
        f()
    };
    harp::test::r_test(f)
}

pub fn r_test_init() {
    harp::test::r_test_init();
    initialize_ark();
}

pub fn r_test_init_lock() -> MutexGuard<'static, ()> {
    r_test_init();
    TEST_LOCK.lock().unwrap()
}

static INIT: Once = Once::new();

fn initialize_ark() {
    INIT.call_once(|| {
        // Initialize the positron module so tests can use them.
        // Routines are already registered by `harp::test::r_test()`.
        modules::initialize().unwrap();
    });
}

pub fn point_from_cursor(x: &str) -> (String, Point) {
    let lines = x.split("\n").collect::<Vec<&str>>();

    // i.e. looking for `@` in something like `fn(x = @1, y = 2)`, and it treats the
    // `@` as the cursor position
    let cursor = b'@';

    for (line_row, line) in lines.into_iter().enumerate() {
        for (char_column, char) in line.as_bytes().into_iter().enumerate() {
            if char == &cursor {
                let x = x.replace("@", "");
                let point = Point {
                    row: line_row,
                    column: char_column,
                };
                return (x, point);
            }
        }
    }

    panic!("`x` must include a `@` character!");
}

pub fn socket_rpc_request<'de, RequestType, ReplyType>(
    socket: &socket::comm::CommSocket,
    req: RequestType,
) -> ReplyType
where
    RequestType: Serialize,
    ReplyType: DeserializeOwned,
{
    // Randomly generate a unique ID for this request.
    let id = uuid::Uuid::new_v4().to_string();

    // Serialize the message for the wire
    let json = serde_json::to_value(req).unwrap();
    println!("--> {:?}", json);

    // Convert the request to a CommMsg and send it.
    let msg = CommMsg::Rpc(id, json);
    socket.incoming_tx.send(msg).unwrap();
    let msg = socket
        .outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    // Extract the reply from the CommMsg.
    match msg {
        CommMsg::Rpc(_id, value) => {
            println!("<-- {:?}", value);
            let reply = serde_json::from_value(value).unwrap();
            reply
        },
        _ => panic!("Unexpected Comm Message"),
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use crate::fixtures::point_from_cursor;

    #[test]
    #[rustfmt::skip]
    fn test_point_from_cursor() {
        let (text, point) = point_from_cursor("1@ + 2");
        assert_eq!(text, "1 + 2".to_string());
        assert_eq!(point, Point::new(0, 1));

        let text =
"fn(
  arg =@ 3
)";
        let expect =
"fn(
  arg = 3
)";
        let (text, point) = point_from_cursor(text);
        assert_eq!(text, expect);
        assert_eq!(point, Point::new(1, 7));
    }
}
