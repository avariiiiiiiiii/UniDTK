use super::{hwid, sanitize_model};

#[test]
fn the_device_id_is_stable_per_provider_and_differs_between_providers() {
    let a = hwid("1234-abcd", "Sub.Example.com");
    assert_eq!(a, hwid("1234-abcd", "sub.example.com"), "the host is compared without case");
    assert_ne!(a, hwid("1234-abcd", "other.example.com"), "two providers cannot match one machine");
    assert_ne!(a, hwid("9999-ffff", "sub.example.com"));
    // Remnawave ignores an x-hwid outside ^[a-zA-Z0-9=-]{10,64}$ (docs.rw, HWID device limit).
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
}

// Every Windows install has a MachineGuid; reading it from the 64-bit view is what keeps
// the id from falling back to a random one.
#[cfg(windows)]
#[test]
fn the_machine_id_and_the_windows_version_are_read_from_the_registry() {
    let guid = super::registry::string(r"SOFTWARE\Microsoft\Cryptography", "MachineGuid").expect("MachineGuid");
    assert_eq!(guid.len(), 36, "a GUID");
    assert!(super::registry::dword(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentMajorVersionNumber").is_some());
}

#[test]
fn the_model_header_is_printable_and_never_an_oem_placeholder() {
    assert_eq!(sanitize_model("ZenBook UX435EA_UX435EA"), "ZenBook UX435EA_UX435EA");
    assert_eq!(sanitize_model("To Be Filled By O.E.M."), "PC");
    assert_eq!(sanitize_model("System Product Name"), "PC");
    assert_eq!(sanitize_model("  "), "PC");
    assert_eq!(sanitize_model("Bad\r\nHeader: x"), "BadHeader: x");
    assert!(sanitize_model(&"x".repeat(200)).len() <= 64);
}
