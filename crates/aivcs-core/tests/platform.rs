use aivcs_core::domain::Platform;

#[test]
fn test_platform_detect() {
    let p = Platform::detect();
    if cfg!(target_os = "macos") {
        assert_eq!(p, Platform::MacOS);
    }
}
