#![no_main]

use libfuzzer_sys::fuzz_mutator;
use libfuzzer_sys::fuzz_target;
use libfuzzer_sys::fuzzer_mutate;
use oak_db::fuzz::execute_json;
use oak_db::fuzz::mutate_json;
use oak_db::fuzz::Runner;

thread_local! {
    static RUNNER: Runner = Runner::open();
}

fuzz_target!(|data: &[u8]| {
    RUNNER.with(|runner| execute_json(runner, data));
});

fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    match mutate_json(data, size, max_size, seed) {
        Some(mutated) => mutated,
        None => fuzzer_mutate(data, size, max_size),
    }
});
