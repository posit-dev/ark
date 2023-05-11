//
// event.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::sync::atomic::AtomicI32;
use std::sync::Mutex;

static ID: AtomicI32 = AtomicI32::new(0);

#[derive(Default)]
pub struct Event<T> {
    listeners: Mutex<HashMap<i32, Box<dyn Fn(&T) + Send>>>,
}

impl<T> Event<T> {
    pub fn emit(&self, data: impl Into<T>) {
        let data = data.into();
        let mut listeners = self.listeners.lock().unwrap();
        for listener in listeners.iter_mut() {
            listener.1(&data);
        }
    }

    pub fn listen(&self, callback: impl Fn(&T) + Send + 'static) -> i32 {
        let id = ID.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let mut listeners = self.listeners.lock().unwrap();
        listeners.insert(id, Box::new(callback));
        return id;
    }

    pub fn remove(&self, id: i32) {
        let mut listeners = self.listeners.lock().unwrap();
        listeners.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signals() {
        #[derive(Default)]
        pub struct Events {
            number: Event<i32>,
            string: Event<String>,
        }

        let signals = Events::default();

        // call with a number
        signals.number.listen(|number| {
            assert!(*number == 42);
        });

        signals.number.emit(42);

        // call with a string
        signals.string.listen(|string| {
            assert!(*string == "hello");
        });

        signals.string.emit("hello");

        // add and remove a signal
        let id = signals.string.listen(|string| {
            assert!(*string == "goodbye");
        });

        signals.string.remove(id);
        signals.string.emit("hello");
    }
}
