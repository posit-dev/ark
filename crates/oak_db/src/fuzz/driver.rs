//! Translates between libFuzzer's byte buffers and [`Scenario`].

use mutatis::Session;

use crate::fuzz::mutate::ScenarioMutator;
use crate::fuzz::run::Runner;
use crate::fuzz::scenario::Scenario;

/// Runs the scenario encoded in `data`, letting a panic propagate so libFuzzer
/// records a crash.
///
/// Input that does not decode is discarded rather than reported. The cached
/// corpus is restored across scenario format changes, so a stale entry costs one
/// run instead of failing the exploration.
pub fn execute_json(runner: &Runner, data: &[u8]) {
    let Ok(scenario) = Scenario::from_json(data) else {
        return;
    };
    runner.execute(&scenario);
}

/// Mutates the scenario encoded in `data[..size]`, writes it back to the front
/// of `data`, and returns the new length. `None` asks the caller to fall back to
/// `libfuzzer_sys::fuzzer_mutate()`, which only the target crate can call.
///
/// `data` is `max(size, max_size)` bytes long, the capacity libFuzzer gives a
/// custom mutator.
pub fn mutate_json(data: &mut [u8], size: usize, max_size: usize, seed: u32) -> Option<usize> {
    let Ok(mut scenario) = Scenario::from_json(&data[..size]) else {
        return None;
    };

    // `size > max_size` is libFuzzer asking for a smaller test case, e.g. for
    // `-minimize_crash`; shrinking must be requested in that direction only.
    let mut session = Session::new().seed(u64::from(seed)).shrink(max_size < size);
    if session
        .mutate_with(&mut ScenarioMutator, &mut scenario)
        .is_err()
    {
        return None;
    }

    let Ok(json) = scenario.to_json() else {
        return None;
    };

    // libFuzzer truncates a longer result to `max_size`, which would leave JSON
    // that no longer parses. Decline the mutation instead.
    let bytes = json.as_bytes();
    if bytes.len() > max_size {
        return None;
    }

    data[..bytes.len()].copy_from_slice(bytes);
    Some(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::mutate_json;
    use crate::fuzz::seed_corpus;
    use crate::fuzz::Scenario;

    /// Matches the `-max_len` of `just fuzz-explore`.
    const MAX_LEN: usize = 32768;

    /// Fills a `max(size, max_size)` buffer the way libFuzzer does.
    fn buffer(scenario: &Scenario, max_size: usize) -> (Vec<u8>, usize) {
        let json = scenario.to_json().unwrap();
        let size = json.len();
        let mut data = vec![0; size.max(max_size)];
        data[..size].copy_from_slice(json.as_bytes());
        (data, size)
    }

    #[test]
    fn test_mutate_json_returns_a_changed_scenario() {
        let scenario = &seed_corpus(0)[0];
        let (mut data, size) = buffer(scenario, MAX_LEN);

        let mutated = mutate_json(&mut data, size, MAX_LEN, 0).unwrap();
        let restored = Scenario::from_json(&data[..mutated]).unwrap();
        assert_ne!(restored.render(), scenario.render());
    }

    /// libFuzzer feeds a mutated input back through the mutator, so output it
    /// cannot decode would stall the search on `fuzzer_mutate()`.
    #[test]
    fn test_mutate_json_accepts_its_own_output() {
        const ROUNDS: u32 = 64;

        let scenario = &seed_corpus(0)[0];
        let (mut data, mut size) = buffer(scenario, MAX_LEN);

        for seed in 0..ROUNDS {
            size = mutate_json(&mut data, size, MAX_LEN, seed).unwrap();
            assert!(size <= MAX_LEN);
            assert!(Scenario::from_json(&data[..size]).is_ok());
        }
    }

    #[test]
    fn test_mutate_json_declines_undecodable_input() {
        let mut data = vec![0; MAX_LEN];
        data[..8].copy_from_slice(b"not json");

        assert_eq!(mutate_json(&mut data, 8, MAX_LEN, 0), None);
    }

    /// `-minimize_crash` lowers `max_size` below any scenario's serialized form.
    #[test]
    fn test_mutate_json_declines_a_result_that_does_not_fit() {
        let scenario = &seed_corpus(0)[0];
        let (mut data, size) = buffer(scenario, 8);

        assert_eq!(mutate_json(&mut data, size, 8, 0), None);
    }
}
