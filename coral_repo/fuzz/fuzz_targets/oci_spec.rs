#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = coral_oci::state::validate_container_id(text);
    }
    if let Ok(spec) = serde_json::from_slice::<coral_oci::OciSpec>(data) {
        let _ = spec.validate();
    }
});
