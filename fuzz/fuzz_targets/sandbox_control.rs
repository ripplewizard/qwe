#![no_main]

use libfuzzer_sys::fuzz_target;
use qwe_runtime::PaneControlDecoder;

fuzz_target!(|data: &[u8]| {
    let split = data.first().copied().map_or(0, usize::from);
    let split = split.min(data.len());
    let mut decoder = PaneControlDecoder::default();
    let _ = decoder.push(&data[..split]);
    let _ = decoder.push(&data[split..]);
    decoder.finish();
});
