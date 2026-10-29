#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| pmtiles_fuzz::read_archive(data));
