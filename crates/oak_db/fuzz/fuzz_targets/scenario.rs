#![no_main]

use libfuzzer_sys::fuzz_mutator;
use libfuzzer_sys::fuzz_target;
use libfuzzer_sys::fuzzer_mutate;
use mutatis::Session;
use oak_db::fuzz::Runner;
use oak_db::fuzz::Scenario;
use oak_db::fuzz::ScenarioMutator;

thread_local! {
    static RUNNER: Runner = Runner::open();
}

fuzz_target!(|data: &[u8]| {
    let Ok(scenario) = Scenario::from_json(data) else {
        return;
    };
    RUNNER.with(|runner| runner.execute(&scenario));
});

fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    let Ok(mut scenario) = Scenario::from_json(&data[..size]) else {
        return fuzzer_mutate(data, size, max_size);
    };

    // `size > max_size` is libFuzzer asking for a smaller test case, e.g. for
    // `-minimize_crash`; shrinking must be requested in that direction only.
    let mut session = Session::new().seed(u64::from(seed)).shrink(max_size < size);
    if session
        .mutate_with(&mut ScenarioMutator, &mut scenario)
        .is_err()
    {
        return fuzzer_mutate(data, size, max_size);
    }

    let Ok(json) = scenario.to_json() else {
        return fuzzer_mutate(data, size, max_size);
    };
    let bytes = json.as_bytes();
    if bytes.len() > max_size {
        return fuzzer_mutate(data, size, max_size);
    }
    data[..bytes.len()].copy_from_slice(bytes);
    bytes.len()
});
