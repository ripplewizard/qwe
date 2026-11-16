#![no_main]

use libfuzzer_sys::fuzz_target;
use qwe_runtime::RestrictedTerminalFilter;

fuzz_target!(|data: &[u8]| {
    let split = data.first().copied().map_or(0, usize::from);
    let split = split.min(data.len());
    let mut filter = RestrictedTerminalFilter::new();
    let first = filter.push(&data[..split]);
    let second = filter.push(&data[split..]);
    let tail = filter.finish();
    let _ = (first, second, tail);
});
