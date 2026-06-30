// ── format_os_display tests ─────────────────────────────────────

#[test]
fn test_format_os_display_azure_linux() {
    let result = crate::display_helpers::format_os_display(
        Some("azure-linux-4"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Azure Linux 4.0");
}

#[test]
fn test_format_os_display_azure_linux_gen2() {
    let result = crate::display_helpers::format_os_display(
        Some("azure-linux-4-gen2"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Azure Linux 4.0");
}

#[test]
fn test_format_os_display_microsoftcblmariner() {
    let result = crate::display_helpers::format_os_display(
        Some("MicrosoftCBLMariner"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Azure Linux 4.0");
}

#[test]
fn test_format_os_display_cbl_mariner_alias() {
    let result = crate::display_helpers::format_os_display(
        Some("cbl-mariner-2"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Azure Linux 4.0");
}

#[test]
fn test_format_os_display_azure_linux_variant() {
    let result = crate::display_helpers::format_os_display(
        Some("azure-linux-4-gen2"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Azure Linux 4.0");
}

#[test]
fn test_format_os_display_unknown_offer_falls_back() {
    let result = crate::display_helpers::format_os_display(
        Some("CustomAzureImage"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "CustomAzureImage");
}

#[test]
fn test_format_os_display_debian() {
    let result = crate::display_helpers::format_os_display(
        Some("debian-11"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Debian");
}

#[test]
fn test_format_os_display_rhel() {
    let result =
        crate::display_helpers::format_os_display(Some("RHEL"), &azlin_core::models::OsType::Linux);
    assert_eq!(result, "RHEL");
}

#[test]
fn test_format_os_display_centos() {
    let result = crate::display_helpers::format_os_display(
        Some("CentOS-7"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "CentOS");
}

#[test]
fn test_format_os_display_suse() {
    let result = crate::display_helpers::format_os_display(
        Some("sles-15"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "SUSE");
}

#[test]
fn test_format_os_display_almalinux() {
    let result = crate::display_helpers::format_os_display(
        Some("almalinux-9"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "AlmaLinux");
}

#[test]
fn test_format_os_display_rocky() {
    let result = crate::display_helpers::format_os_display(
        Some("rockylinux-9"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "Rocky Linux");
}

#[test]
fn test_format_os_display_windows_offer() {
    let result = crate::display_helpers::format_os_display(
        Some("WindowsServer"),
        &azlin_core::models::OsType::Windows,
    );
    assert_eq!(result, "Windows");
}

#[test]
fn test_format_os_display_unknown_offer_passthrough() {
    let result = crate::display_helpers::format_os_display(
        Some("CustomImage"),
        &azlin_core::models::OsType::Linux,
    );
    assert_eq!(result, "CustomImage");
}

#[test]
fn test_format_os_display_none_linux() {
    let result =
        crate::display_helpers::format_os_display(None, &azlin_core::models::OsType::Linux);
    assert_eq!(result, "Linux");
}

#[test]
fn test_format_os_display_none_windows() {
    let result =
        crate::display_helpers::format_os_display(None, &azlin_core::models::OsType::Windows);
    assert_eq!(result, "Windows");
}

// ── format_ip_display tests ─────────────────────────────────────

#[test]
fn test_format_ip_display_public_ip() {
    let result = crate::display_helpers::format_ip_display(Some("52.1.2.3"), Some("10.0.0.4"));
    assert_eq!(result, "52.1.2.3 (Pub)");
}

#[test]
fn test_format_ip_display_private_only() {
    let result = crate::display_helpers::format_ip_display(None, Some("10.0.0.4"));
    assert_eq!(result, "10.0.0.4 (Bast)");
}

#[test]
fn test_format_ip_display_no_ip() {
    let result = crate::display_helpers::format_ip_display(None, None);
    assert_eq!(result, "N/A");
}

#[test]
fn test_format_ip_display_public_takes_precedence() {
    // When both exist, public IP should be shown
    let result = crate::display_helpers::format_ip_display(Some("1.2.3.4"), Some("10.0.0.1"));
    assert!(result.contains("(Pub)"));
    assert!(result.contains("1.2.3.4"));
}

// ── parse_vm_size_specs tests ───────────────────────────────────

#[test]
fn test_parse_vm_size_specs_d4s_v3() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_D4s_v3");
    assert_eq!(vcpus, "4");
    assert_eq!(mem, "16 GB"); // D-series: 4 vcpus * 4 GB
}

#[test]
fn test_parse_vm_size_specs_e16as() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_E16as_v5");
    assert_eq!(vcpus, "16");
    assert_eq!(mem, "128 GB"); // E-series: 16 vcpus * 8 GB
}

#[test]
fn test_parse_vm_size_specs_f2s() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_F2s_v2");
    assert_eq!(vcpus, "2");
    assert_eq!(mem, "4 GB"); // F-series: 2 vcpus * 2 GB
}

#[test]
fn test_parse_vm_size_specs_b1s() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_B1s");
    assert_eq!(vcpus, "1");
    assert_eq!(mem, "4 GB"); // B-series: 1 * 4 GB
}

#[test]
fn test_parse_vm_size_specs_m32() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_M32ms_v2");
    assert_eq!(vcpus, "32");
    assert_eq!(mem, "512 GB"); // M-series: 32 * 16 GB
}

#[test]
fn test_parse_vm_size_specs_n6() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_N6s_v3");
    assert_eq!(vcpus, "6");
    assert_eq!(mem, "36 GB"); // N-series: 6 * 6 GB
}

#[test]
fn test_parse_vm_size_specs_l8s() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_L8s_v3");
    assert_eq!(vcpus, "8");
    assert_eq!(mem, "64 GB"); // L-series: 8 * 8 GB
}

#[test]
fn test_parse_vm_size_specs_invalid_format() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("NotAVmSize");
    assert_eq!(vcpus, "-");
    assert_eq!(mem, "-");
}

#[test]
fn test_parse_vm_size_specs_empty() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("");
    assert_eq!(vcpus, "-");
    assert_eq!(mem, "-");
}

#[test]
fn test_parse_vm_size_specs_no_number() {
    let (vcpus, mem) = crate::display_helpers::parse_vm_size_specs("Standard_Ds_v3");
    assert_eq!(vcpus, "-");
    assert_eq!(mem, "-");
}
