//! `azlin gui` — Open a remote desktop on a VM over an SSH tunnel.
//!
//! Azure Linux ships no desktop environment, no VNC server and no RDP server in
//! any of its repositories, so the desktop stack cannot be installed with the
//! package manager. It runs instead as a container on the VM's Docker, put there
//! by `azlin gui install` (see [`crate::cmd_gui_install`]).
//!
//! Workflow:
//! 1. Check local prerequisites (X server, viewer/client)
//! 2. Resolve VM and detect bastion route
//! 3. Probe the VM for the desktop container; if absent, tell the user to run
//!    `azlin gui install` and exit non-zero — never install implicitly
//! 4. Start the container if it exists but is stopped
//! 5. SSH port-forward the desktop port, which is published on the VM's
//!    loopback interface only
//! 6. Launch the local viewer/client
//! 7. Wait for it to exit, then tear the tunnel down

#[allow(unused_imports)]
use super::*;
use anyhow::{Context, Result};
use azlin_core::gui_container::{
    build_detect_script, build_start_script, check_available, parse_detect_output, ContainerState,
    GuiProtocol, GuiStatus, HOST_VNC_PASSWD_PATH, RDP_USERNAME,
};

/// Hard timeout for the remote desktop detection probe.
const GUI_DETECT_TIMEOUT_SECS: u64 = 60;

pub(crate) fn resolve_gui_target_user(requested_user: &str, detected_user: &str) -> String {
    if requested_user != DEFAULT_ADMIN_USERNAME {
        requested_user.to_string()
    } else {
        detected_user.to_string()
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub(crate) async fn dispatch(
    command: azlin_cli::Commands,
    verbose: bool,
    output: &azlin_cli::OutputFormat,
) -> Result<()> {
    let azlin_cli::Commands::Gui {
        action,
        vm_identifier,
        resource_group,
        user,
        key,
        resolution: _resolution,
        depth: _depth,
        yes: _yes,
        minimal,
        app,
    } = command
    else {
        unreachable!()
    };

    if let Some(action) = action {
        return crate::cmd_gui_install::dispatch(action, verbose, output).await;
    }

    // Session shape flags only ever applied to a VNC server installed directly on
    // the host. The containerised desktop owns its own session, so honouring them
    // is impossible; say so rather than silently ignoring them.
    if minimal || app.is_some() {
        eprintln!(
            "warning: --minimal and --app are not supported for the containerised desktop and are ignored."
        );
        eprintln!("         Run an application from inside the desktop session instead.");
    }

    // Step 2: Resolve VM
    let rg = resolve_resource_group(resource_group)?;

    let name = if let Some(n) = vm_identifier {
        n
    } else {
        anyhow::bail!("VM name is required for gui command. Usage: azlin gui <vm-name>");
    };

    let pb = penguin_spinner(&format!("Looking up {}...", name));
    let mut target = resolve_vm_ssh_target(&name, None, Some(rg.clone())).await?;
    target.user = resolve_gui_target_user(&user, &target.user);
    pb.finish_and_clear();
    let config = azlin_core::AzlinConfig::load().unwrap_or_default();
    let effective_key = key.or_else(resolve_ssh_key);
    let (ssh_cmd_prefix, _route_tunnel) = build_gui_ssh_command_prefix(
        &target,
        config.ssh_connect_timeout,
        effective_key.as_deref(),
    )?;

    // Step 3: Detect the desktop container. Absent means "run gui install",
    // never an implicit install.
    let pb = penguin_spinner("Checking for the remote desktop...");
    let status = detect_desktop(&target, effective_key.as_deref());
    pb.finish_and_clear();
    let status = status?;

    if let Err(unavailable) = check_available(&status, &name) {
        anyhow::bail!("{}", unavailable);
    }

    // Step 4: Start it if it is merely stopped.
    if status.container_state == ContainerState::Stopped {
        let pb = penguin_spinner("Starting the remote desktop container...");
        let started = start_desktop(&ssh_cmd_prefix);
        pb.finish_and_clear();
        started?;
    }

    let protocol = status.protocol.unwrap_or(GuiProtocol::Vnc);

    // Local viewer prerequisites are checked only once we know the desktop is
    // actually installed and which protocol it speaks. Checking earlier would
    // mask the actionable "run azlin gui install" error behind a local tooling
    // error, and would demand a VNC viewer even for an RDP desktop.
    check_local_deps(protocol)?;

    let remote_port = status
        .host_port
        .unwrap_or_else(|| azlin_core::gui_container::image_for(protocol).container_port);

    // Step 5: Open the SSH port-forward. The desktop port is bound to the VM's
    // loopback interface, so the tunnel is the only way in.
    let pb = penguin_spinner("Opening the desktop tunnel...");
    let opened = open_desktop_tunnel(&ssh_cmd_prefix, remote_port);
    pb.finish_and_clear();
    let (local_port, tunnel_pids) = opened?;

    // Step 6: Launch the local viewer/client.
    let result = match protocol {
        GuiProtocol::Vnc => {
            println!("Launching VNC viewer (127.0.0.1:{})...", local_port);
            eprintln!("(desktop password set on the VM — not displayed for security)");
            println!("Press Ctrl+C to stop the GUI session.\n");
            launch_viewer(&ssh_cmd_prefix, local_port)
        }
        GuiProtocol::Rdp => launch_rdp_client(&ssh_cmd_prefix, local_port),
    };

    // Step 7: Cleanup. The container is left running so reconnecting is fast;
    // remove it with `azlin gui install <vm> --uninstall`.
    cleanup(&tunnel_pids);

    result
}

// ---------------------------------------------------------------------------
// Desktop detection
// ---------------------------------------------------------------------------

fn detect_desktop(target: &VmSshTarget, key_override: Option<&std::path::Path>) -> Result<GuiStatus> {
    run_detect_with_runner(GUI_DETECT_TIMEOUT_SECS, |script, timeout| {
        crate::dispatch_helpers::run_target_command_with_timeout(
            target, script, timeout, key_override,
        )
    })
}

/// Run the detection probe and parse its output.
///
/// The probe always exits zero, so a non-zero exit means the SSH transport
/// failed and must be reported as such rather than as "not installed".
fn run_detect_with_runner<F>(timeout_secs: u64, mut runner: F) -> Result<GuiStatus>
where
    F: FnMut(&str, u64) -> Result<(i32, String, String)>,
{
    let script = crate::cmd_gui_install::wrap_for_shell(&build_detect_script());
    match runner(&script, timeout_secs) {
        Ok((0, stdout, _)) => Ok(parse_detect_output(&stdout)),
        Ok((code, _, stderr)) => anyhow::bail!(
            "Could not check the VM for a remote desktop (exit {}): {}",
            code,
            azlin_core::sanitizer::sanitize(stderr.trim())
        ),
        Err(err) => anyhow::bail!(
            "Could not check the VM for a remote desktop: {}",
            azlin_core::sanitizer::sanitize(&err.to_string())
        ),
    }
}

fn start_desktop(ssh_cmd_prefix: &[String]) -> Result<()> {
    let script = crate::cmd_gui_install::wrap_for_shell(&build_start_script());
    let (code, _, stderr) = run_ssh_command_full(ssh_cmd_prefix, &script)?;
    if code != 0 {
        anyhow::bail!(
            "The remote desktop container exists but could not be started: {}\n             Inspect it on the VM with: docker logs azlin-gui",
            azlin_core::sanitizer::sanitize(stderr.trim())
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Local prerequisite checks
// ---------------------------------------------------------------------------

fn check_local_deps(protocol: GuiProtocol) -> Result<()> {
    // RDP is served by a local RDP client, which azlin locates separately and
    // for which it can fall back to printing manual instructions. There is
    // nothing to require here.
    if protocol == GuiProtocol::Rdp {
        return Ok(());
    }

    // Check for X server availability
    let display_set = std::env::var("DISPLAY")
        .map(|d| !d.is_empty())
        .unwrap_or(false);
    let x_socket_exists = std::path::Path::new("/tmp/.X11-unix/X0").exists();

    if !display_set && !x_socket_exists {
        eprintln!("Warning: No X server detected.");
        eprintln!(
            "  WSLg should be available in WSL2 by default. Restart WSL if DISPLAY is not set."
        );
        eprintln!("  Alternatively, install an X server like VcXsrv or Xming.");
        // Not fatal — vncviewer may still work if DISPLAY gets set before launch
    }

    // Check for vncviewer
    let has_vncviewer = std::process::Command::new("which")
        .arg("vncviewer")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !has_vncviewer {
        anyhow::bail!(
            "vncviewer not found. Install it with:\n  \
             Debian/Ubuntu: sudo apt-get install -y tigervnc-viewer tigervnc-common\n  \
             Fedora/RHEL:   sudo dnf install -y tigervnc\n  \
             macOS:         brew install --cask tigervnc-viewer"
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// SSH prefix builders
// ---------------------------------------------------------------------------

/// Build an SSH command prefix for direct connection to a public-IP VM.
#[cfg(test)]
fn build_direct_ssh_prefix(ip: &str, user: &str, key: Option<&std::path::Path>) -> Vec<String> {
    let config = azlin_core::AzlinConfig::load().unwrap_or_default();
    let mut prefix = vec!["ssh".to_string()];
    prefix.extend(crate::ssh_arg_helpers::build_ssh_prefix(
        ip,
        user,
        config.ssh_connect_timeout,
    ));
    if let Some(k) = key {
        crate::ssh_arg_helpers::inject_identity_key_before_destination(&mut prefix, k);
    }
    prefix
}

fn build_gui_ssh_command_prefix(
    target: &VmSshTarget,
    connect_timeout: u64,
    key_override: Option<&std::path::Path>,
) -> Result<(
    Vec<String>,
    Option<crate::bastion_tunnel::ScopedBastionTunnel>,
)> {
    let (routed_prefix, tunnel) =
        crate::dispatch_helpers::build_routed_ssh_prefix(target, connect_timeout, key_override)?;
    let mut ssh_cmd_prefix = Vec::with_capacity(routed_prefix.len() + 1);
    ssh_cmd_prefix.push("ssh".to_string());
    ssh_cmd_prefix.extend(routed_prefix);
    Ok((ssh_cmd_prefix, tunnel))
}

// ---------------------------------------------------------------------------
// Desktop tunnel (SSH -L port forwarding)
// ---------------------------------------------------------------------------

/// Build `ssh -N -L <local>:localhost:<remote>` from an existing SSH prefix.
///
/// `remote_port` is the loopback port the desktop container publishes on the VM,
/// so the same code path serves both VNC (5901) and RDP (3389).
fn build_desktop_tunnel_args(
    ssh_cmd_prefix: &[String],
    local_port: u16,
    remote_port: u16,
) -> Result<Vec<String>> {
    let mut args: Vec<String> = Vec::new();

    // prefix[0] = "ssh", prefix[1..] = options + user@host
    if ssh_cmd_prefix.len() < 2 {
        anyhow::bail!("SSH command prefix must include a destination");
    }

    for arg in &ssh_cmd_prefix[1..ssh_cmd_prefix.len() - 1] {
        args.push(arg.clone());
    }
    args.push("-N".to_string());
    args.push("-L".to_string());
    args.push(format!("{}:localhost:{}", local_port, remote_port));
    // user@host is the last element
    args.push(ssh_cmd_prefix.last().unwrap().clone());

    Ok(args)
}

fn open_desktop_tunnel(ssh_cmd_prefix: &[String], remote_port: u16) -> Result<(u16, Vec<u32>)> {
    let local_port = crate::pick_unused_local_port()?;
    let args = build_desktop_tunnel_args(ssh_cmd_prefix, local_port, remote_port)?;

    let mut child = std::process::Command::new("ssh")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to spawn SSH port-forward for the remote desktop")?;

    let pid = child.id();
    if let Err(error) = crate::bastion_tunnel::wait_for_process_tree_listener(
        local_port,
        pid,
        std::time::Duration::from_secs(10),
        "desktop tunnel",
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context(format!(
            "Desktop tunnel failed to listen on 127.0.0.1:{}",
            local_port
        ));
    }
    std::mem::forget(child);

    Ok((local_port, vec![pid]))
}

// ---------------------------------------------------------------------------
// VNC viewer launch
// ---------------------------------------------------------------------------

fn build_vnc_viewer_args(passwd_file: &std::path::Path, local_port: u16) -> Vec<String> {
    vec![
        "-SecurityTypes".to_string(),
        "VncAuth".to_string(),
        "-passwd".to_string(),
        passwd_file.display().to_string(),
        format!("127.0.0.1:{}", local_port),
    ]
}

fn launch_viewer(ssh_cmd_prefix: &[String], local_port: u16) -> Result<()> {
    // The container's own `vncpasswd` blob was copied onto the VM by
    // `azlin gui install`; fetch it so the local viewer can authenticate.
    let passwd_b64 = run_ssh_command(
        ssh_cmd_prefix,
        &format!("base64 < {}", HOST_VNC_PASSWD_PATH),
    )
    .context(
        "Could not read the desktop password from the VM. Re-run `azlin gui install <vm>` to \
         regenerate it.",
    )?;
    let passwd_bytes = base64_decode(passwd_b64.trim())?;

    // Write to a temp file with restricted permissions from creation (no TOCTOU window)
    let tmp_dir = std::env::temp_dir();
    let passwd_file = tmp_dir.join(format!("azlin_vnc_passwd_{}", std::process::id()));
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&passwd_file)
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(&passwd_bytes)
                })
                .context("Failed to write temporary VNC passwd file")?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&passwd_file, &passwd_bytes)
                .context("Failed to write temporary VNC passwd file")?;
        }
    }

    // Ensure DISPLAY is set for the viewer
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let effective_display = if display.is_empty() {
        // Check if X socket exists (WSLg)
        if std::path::Path::new("/tmp/.X11-unix/X0").exists() {
            ":0".to_string()
        } else {
            display
        }
    } else {
        display
    };

    let mut cmd = std::process::Command::new("vncviewer");
    cmd.args(build_vnc_viewer_args(&passwd_file, local_port));

    if !effective_display.is_empty() {
        cmd.env("DISPLAY", &effective_display);
    }

    let launch_result = cmd.status().context("Failed to launch vncviewer");

    // Clean up temp passwd file unconditionally (before propagating any error)
    if let Err(e) = std::fs::remove_file(&passwd_file) {
        eprintln!(
            "warning: failed to remove temp VNC passwd file {}: {e}",
            passwd_file.display()
        );
    }

    let status = launch_result?;

    if !status.success() {
        anyhow::bail!(
            "vncviewer exited with status {}",
            status.code().unwrap_or(-1)
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// RDP client launch
// ---------------------------------------------------------------------------

/// Local RDP clients azlin knows how to drive, in preference order.
const RDP_CLIENTS: &[&str] = &["xfreerdp3", "xfreerdp", "mstsc.exe", "mstsc"];

/// Find the first available local RDP client.
fn find_rdp_client(is_available: impl Fn(&str) -> bool) -> Option<&'static str> {
    RDP_CLIENTS.iter().copied().find(|c| is_available(c))
}

/// Build the argument list for a given RDP client binary.
fn build_rdp_client_args(client: &str, local_port: u16, username: &str) -> Vec<String> {
    if client.starts_with("mstsc") {
        // mstsc takes only the endpoint; it prompts for credentials.
        vec![format!("/v:127.0.0.1:{}", local_port)]
    } else {
        vec![
            format!("/v:127.0.0.1:{}", local_port),
            format!("/u:{}", username),
            "/cert:ignore".to_string(),
            "/dynamic-resolution".to_string(),
        ]
    }
}

/// Instructions printed when no local RDP client is available.
fn rdp_manual_instructions(local_port: u16, username: &str) -> String {
    format!(
        "The RDP tunnel is open on 127.0.0.1:{local_port}.\n         Connect with any RDP client using:\n           host:     127.0.0.1:{local_port}\n           username: {username}\n           password: run `azlin gui install <vm>` output, or read ~/.azlin/gui/rdppasswd on the VM\n\n         Examples:\n           xfreerdp /v:127.0.0.1:{local_port} /u:{username} /cert:ignore\n           mstsc /v:127.0.0.1:{local_port}\n           macOS: open Microsoft Remote Desktop and add PC 127.0.0.1:{local_port}\n\n         Press Ctrl+C to close the tunnel."
    )
}

fn launch_rdp_client(ssh_cmd_prefix: &[String], local_port: u16) -> Result<()> {
    let username = RDP_USERNAME;

    let Some(client) = find_rdp_client(|c| {
        std::process::Command::new("which")
            .arg(c)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }) else {
        println!("{}", rdp_manual_instructions(local_port, username));
        // Hold the tunnel open until interrupted so the printed endpoint is usable.
        wait_for_interrupt();
        return Ok(());
    };

    // Surface the password so the user can paste it into the client prompt. It
    // never leaves the SSH channel and is not written to disk locally.
    match run_ssh_command(ssh_cmd_prefix, "cat \"$HOME/.azlin/gui/rdppasswd\"") {
        Ok(password) if !password.trim().is_empty() => {
            println!("RDP login: {} / {}", username, password.trim());
        }
        _ => {
            eprintln!(
                "warning: could not read the RDP password from the VM (~/.azlin/gui/rdppasswd)."
            );
            eprintln!("         Re-run `azlin gui install <vm> --protocol rdp` to regenerate it.");
        }
    }

    println!("Launching {} (127.0.0.1:{})...", client, local_port);
    println!("Press Ctrl+C to stop the GUI session.\n");

    let status = std::process::Command::new(client)
        .args(build_rdp_client_args(client, local_port, username))
        .status()
        .with_context(|| format!("Failed to launch {}", client))?;

    if !status.success() {
        anyhow::bail!("{} exited with status {}", client, status.code().unwrap_or(-1));
    }

    Ok(())
}

/// Block until the user interrupts, keeping a tunnel usable meanwhile.
fn wait_for_interrupt() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

/// Tear down the local tunnel processes.
///
/// The desktop container is intentionally left running so that reconnecting is
/// fast; `azlin gui install <vm> --uninstall` removes it.
fn cleanup(pids: &[u32]) {
    for pid in pids {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

// ---------------------------------------------------------------------------
// SSH helpers
// ---------------------------------------------------------------------------

/// Run a command on the remote VM via SSH, returning stdout.
fn run_ssh_command(ssh_cmd_prefix: &[String], remote_cmd: &str) -> Result<String> {
    let (code, stdout, stderr) = run_ssh_command_full(ssh_cmd_prefix, remote_cmd)?;
    if code != 0 {
        anyhow::bail!("SSH command failed (exit {}): {}", code, stderr);
    }
    Ok(stdout)
}

/// Run a command on the remote VM via SSH, returning (exit_code, stdout, stderr).
fn run_ssh_command_full(
    ssh_cmd_prefix: &[String],
    remote_cmd: &str,
) -> Result<(i32, String, String)> {
    if ssh_cmd_prefix.is_empty() {
        anyhow::bail!("Empty SSH command prefix");
    }

    let output = std::process::Command::new(&ssh_cmd_prefix[0])
        .args(&ssh_cmd_prefix[1..])
        .arg(remote_cmd)
        .output()
        .context("Failed to execute SSH command")?;

    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Validate resolution string format (WIDTHxHEIGHT).
pub(crate) fn is_valid_resolution(res: &str) -> bool {
    let parts: Vec<&str> = res.split('x').collect();
    if parts.len() != 2 {
        return false;
    }
    parts[0].parse::<u32>().is_ok() && parts[1].parse::<u32>().is_ok()
}

/// Simple base64 decoder (avoids adding a dependency).
/// Handles standard base64 alphabet with optional padding.
fn base64_decode(input: &str) -> Result<Vec<u8>> {
    // Use openssl or a subprocess to decode if available, otherwise manual decode
    let output = std::process::Command::new("base64")
        .arg("-d")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(input.as_bytes())?;
            }
            child.wait_with_output()
        })
        .context("Failed to decode base64 VNC password")?;

    if !output.status.success() {
        anyhow::bail!("base64 decode failed");
    }

    Ok(output.stdout)
}

/// Build SSH arguments for X11 forwarding (used by connect --x11).
#[allow(dead_code)]
pub fn build_x11_ssh_args() -> Vec<String> {
    vec!["-Y".to_string()]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_resolution() {
        assert!(is_valid_resolution("1920x1080"));
        assert!(is_valid_resolution("1280x720"));
        assert!(is_valid_resolution("3840x2160"));
    }

    #[test]
    fn test_invalid_resolution() {
        assert!(!is_valid_resolution("1920"));
        assert!(!is_valid_resolution("1920x"));
        assert!(!is_valid_resolution("x1080"));
        assert!(!is_valid_resolution("abc"));
        assert!(!is_valid_resolution("1920x1080x32"));
        assert!(!is_valid_resolution(""));
    }

    #[test]
    fn test_direct_ssh_prefix_no_key() {
        let prefix = build_direct_ssh_prefix("10.0.0.1", "testuser", None);
        assert_eq!(prefix[0], "ssh");
        assert!(prefix.contains(&"-o".to_string()));
        assert!(prefix.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert_eq!(prefix.last().unwrap(), "testuser@10.0.0.1");
    }

    #[test]
    fn test_direct_ssh_prefix_with_key() {
        let key_path = std::path::Path::new("/home/user/.ssh/id_rsa");
        let prefix = build_direct_ssh_prefix("10.0.0.1", "testuser", Some(key_path));
        assert!(prefix.contains(&"IdentitiesOnly=yes".to_string()));
        assert!(prefix.contains(&"-i".to_string()));
        assert!(prefix.contains(&"/home/user/.ssh/id_rsa".to_string()));
        assert_eq!(prefix.last().unwrap(), "testuser@10.0.0.1");
    }

    #[test]
    fn test_gui_routed_ssh_command_prefix_starts_with_ssh_binary() {
        let target = VmSshTarget {
            vm_name: "simard".to_string(),
            ip: "1.2.3.4".to_string(),
            user: "azureuser".to_string(),
            ssh_key_path: None,
            allow_preferred_key_fallback: false,
            bastion: None,
        };

        let (prefix, tunnel) = build_gui_ssh_command_prefix(&target, 30, None).unwrap();
        assert!(tunnel.is_none());
        assert_eq!(prefix.first().map(String::as_str), Some("ssh"));
        assert!(prefix.contains(&"BatchMode=yes".to_string()));
        assert_eq!(prefix.last().map(String::as_str), Some("azureuser@1.2.3.4"));
    }

    #[test]
    fn test_build_x11_ssh_args() {
        let args = build_x11_ssh_args();
        assert_eq!(args, vec!["-Y".to_string()]);
    }

    #[test]
    fn test_resolve_gui_target_user_honors_non_default_override() {
        assert_eq!(
            resolve_gui_target_user("customuser", "azureuser"),
            "customuser"
        );
        assert_eq!(
            resolve_gui_target_user(DEFAULT_ADMIN_USERNAME, "vmadmin"),
            "vmadmin"
        );
    }

    // -- tunnel --------------------------------------------------------------

    #[test]
    fn test_build_desktop_tunnel_args_use_requested_ports() {
        let args = build_desktop_tunnel_args(
            &[
                "ssh".to_string(),
                "-i".to_string(),
                "/tmp/test-key".to_string(),
                "azureuser@10.0.0.5".to_string(),
            ],
            41234,
            5901,
        )
        .unwrap();

        assert!(args.contains(&"-N".to_string()));
        assert!(args.contains(&"-L".to_string()));
        assert!(args.contains(&"41234:localhost:5901".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("azureuser@10.0.0.5"));
    }

    /// RDP reuses the identical SSH tunnel mechanism, only the remote port differs.
    #[test]
    fn test_build_desktop_tunnel_args_support_rdp_port() {
        let args = build_desktop_tunnel_args(
            &["ssh".to_string(), "azureuser@10.0.0.5".to_string()],
            41999,
            3389,
        )
        .unwrap();
        assert!(args.contains(&"41999:localhost:3389".to_string()));
    }

    /// The forward must always target the VM's loopback interface, matching the
    /// container's loopback-only port publication.
    #[test]
    fn test_desktop_tunnel_forwards_to_loopback_only() {
        for remote_port in [5901u16, 3389] {
            let args = build_desktop_tunnel_args(
                &["ssh".to_string(), "azureuser@10.0.0.5".to_string()],
                40000,
                remote_port,
            )
            .unwrap();
            let spec = args
                .iter()
                .position(|a| a == "-L")
                .map(|i| args[i + 1].clone())
                .unwrap();
            assert!(
                spec.ends_with(&format!(":localhost:{remote_port}")),
                "forward must target localhost, got {spec}"
            );
        }
    }

    #[test]
    fn test_build_desktop_tunnel_args_require_destination() {
        let err = build_desktop_tunnel_args(&["ssh".to_string()], 41234, 5901).unwrap_err();
        assert!(err.to_string().contains("must include a destination"));
    }

    #[test]
    fn test_build_vnc_viewer_args_use_requested_local_port() {
        let args = build_vnc_viewer_args(std::path::Path::new("/tmp/passwd"), 41234);
        assert_eq!(
            args,
            vec![
                "-SecurityTypes".to_string(),
                "VncAuth".to_string(),
                "-passwd".to_string(),
                "/tmp/passwd".to_string(),
                "127.0.0.1:41234".to_string(),
            ]
        );
    }

    // -- detection -----------------------------------------------------------

    #[test]
    fn test_detect_parses_a_running_container() {
        let status = run_detect_with_runner(60, |_, _| {
            Ok((
                0,
                "docker_present=true\ndocker_usable=true\ncontainer_state=running\nprotocol=rdp\nhost_port=3389\n"
                    .to_string(),
                String::new(),
            ))
        })
        .unwrap();
        assert_eq!(status.container_state, ContainerState::Running);
        assert_eq!(status.protocol, Some(GuiProtocol::Rdp));
        assert_eq!(status.host_port, Some(3389));
    }

    /// A transport failure must never be misreported as "desktop not installed".
    #[test]
    fn test_detect_transport_failure_is_not_confused_with_missing_desktop() {
        let err = run_detect_with_runner(60, |_, _| {
            Ok((255, String::new(), "ssh: connect: timed out".to_string()))
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("Could not check the VM"));
        assert!(!err.contains("azlin gui install"));
    }

    #[test]
    fn test_detect_runs_under_a_login_shell_with_the_given_timeout() {
        let mut seen_script = String::new();
        let mut seen_timeout = 0;
        let _ = run_detect_with_runner(42, |script, timeout| {
            seen_script = script.to_string();
            seen_timeout = timeout;
            Ok((0, "docker_present=true".to_string(), String::new()))
        });
        assert!(seen_script.starts_with("bash -lc "));
        assert_eq!(seen_timeout, 42);
    }

    /// The connect path must refuse to run and point at `gui install`.
    #[test]
    fn test_missing_desktop_tells_the_user_to_install_it() {
        let status = run_detect_with_runner(60, |_, _| {
            Ok((
                0,
                "docker_present=true\ndocker_usable=true\ncontainer_state=missing\n".to_string(),
                String::new(),
            ))
        })
        .unwrap();
        let err = check_available(&status, "my-vm").unwrap_err();
        assert!(err.to_string().contains("azlin gui install my-vm"));
    }

    // -- RDP client ----------------------------------------------------------

    #[test]
    fn test_rdp_client_preference_order() {
        assert_eq!(find_rdp_client(|_| true), Some("xfreerdp3"));
        assert_eq!(find_rdp_client(|c| c == "mstsc"), Some("mstsc"));
        assert_eq!(find_rdp_client(|_| false), None);
    }

    #[test]
    fn test_rdp_client_args_target_the_local_tunnel() {
        let args = build_rdp_client_args("xfreerdp", 41234, "abc");
        assert!(args.contains(&"/v:127.0.0.1:41234".to_string()));
        assert!(args.contains(&"/u:abc".to_string()));
        assert!(args.contains(&"/cert:ignore".to_string()));
    }

    #[test]
    fn test_mstsc_args_omit_unsupported_switches() {
        let args = build_rdp_client_args("mstsc", 41234, "abc");
        assert_eq!(args, vec!["/v:127.0.0.1:41234".to_string()]);
    }

    /// The RDP endpoint is only ever the local end of the SSH tunnel.
    #[test]
    fn test_rdp_never_targets_a_public_endpoint() {
        for client in ["xfreerdp3", "xfreerdp", "mstsc"] {
            for arg in build_rdp_client_args(client, 41234, "abc") {
                if arg.starts_with("/v:") {
                    assert_eq!(arg, "/v:127.0.0.1:41234");
                }
            }
        }
    }

    #[test]
    fn test_rdp_manual_instructions_are_actionable() {
        let text = rdp_manual_instructions(41234, "abc");
        assert!(text.contains("127.0.0.1:41234"));
        assert!(text.contains("xfreerdp"));
        assert!(text.contains("mstsc"));
        assert!(text.contains("Microsoft Remote Desktop"));
        assert!(text.contains("abc"));
    }
    #[test]
    fn rdp_never_requires_a_local_vnc_viewer() {
        // An RDP desktop is reached with an RDP client; requiring vncviewer
        // would make `azlin gui` unusable on a correct RDP setup.
        assert!(check_local_deps(GuiProtocol::Rdp).is_ok());
    }

}
