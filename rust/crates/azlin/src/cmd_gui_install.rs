//! `azlin gui install` — install the containerised remote desktop on a VM.
//!
//! Azure Linux ships no desktop environment, no VNC server and no RDP server in
//! its repositories, so the desktop stack runs as a container on the VM's
//! Docker (installed by azlin's bootstrap as `moby-engine` + `docker-cli`).
//!
//! This module is deliberately thin. Every decision — which image, which port,
//! how the container is created, how failures are classified — lives in
//! [`azlin_core::gui_container`], which is pure and unit-tested without a VM.
//! Here we only resolve the VM, run the generated script over the existing SSH
//! or bastion route, and surface the result.
//!
//! # Security
//!
//! No Azure network security group rule is created, modified or read. The
//! desktop port is published on the VM's loopback interface only and is reached
//! exclusively through azlin's SSH tunnel.

#[allow(unused_imports)]
use super::*;
use anyhow::Result;
use azlin_core::gui_container::{
    build_install_script, build_uninstall_script, describe_install_failure, DesktopGeometry,
    GuiInstallPlan,
};

/// Hard timeout for the install phase. Pulling a desktop image is the slow part.
const GUI_INSTALL_TIMEOUT_SECS: u64 = 1_200;

/// Timeout for the (fast) uninstall phase.
const GUI_UNINSTALL_TIMEOUT_SECS: u64 = 120;

pub(crate) async fn dispatch(
    action: azlin_cli::GuiAction,
    _verbose: bool,
    _output: &azlin_cli::OutputFormat,
) -> Result<()> {
    let azlin_cli::GuiAction::Install {
        vm_identifier,
        protocol,
        resource_group,
        user,
        key,
        resolution,
        depth,
        uninstall,
        yes: _yes,
    } = action;

    if !crate::cmd_gui::is_valid_resolution(&resolution) {
        anyhow::bail!(
            "Invalid resolution '{}'. Expected format: WIDTHxHEIGHT (e.g. 1920x1080)",
            resolution
        );
    }

    let Some(name) = vm_identifier else {
        anyhow::bail!(
            "VM name is required. Usage: azlin gui install <vm-name> [--protocol vnc|rdp]"
        );
    };

    let rg = resolve_resource_group(resource_group)?;

    let pb = penguin_spinner(&format!("Looking up {}...", name));
    let mut target = resolve_vm_ssh_target(&name, None, Some(rg)).await?;
    target.user = crate::cmd_gui::resolve_gui_target_user(&user, &target.user);
    pb.finish_and_clear();

    let effective_key = key.or_else(resolve_ssh_key);

    if uninstall {
        return run_uninstall(&target, effective_key.as_deref());
    }

    let plan = GuiInstallPlan::new(
        protocol.to_core(),
        DesktopGeometry {
            resolution: resolution.clone(),
            depth,
        },
    );

    println!(
        "Installing the {} desktop on {} using {}",
        plan.protocol, name, plan.image.reference
    );
    println!(
        "  port {} is published on the VM's loopback interface only; connect via `azlin gui {}`",
        plan.host_port, name
    );

    let pb = penguin_spinner("Installing remote desktop container (this can take a few minutes)...");
    let outcome = run_install_with_runner(&plan, GUI_INSTALL_TIMEOUT_SECS, |script, timeout| {
        crate::dispatch_helpers::run_target_command_with_timeout(
            &target,
            script,
            timeout,
            effective_key.as_deref(),
        )
    });
    pb.finish_and_clear();

    match outcome? {
        InstallOutcome::AlreadyInstalled => {
            println!(
                "Remote desktop already installed ({}, {}).",
                plan.protocol, plan.image.reference
            );
        }
        InstallOutcome::Installed => {
            println!(
                "Remote desktop installed ({}, {}).",
                plan.protocol, plan.image.reference
            );
        }
    }
    println!("  clients: {}", plan.image.client_support);
    println!("  connect: azlin gui {}", name);

    Ok(())
}

/// Result of a successful install run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    /// A matching container was already present and was (re)started.
    AlreadyInstalled,
    /// The container was created.
    Installed,
}

/// Run the install script through `runner` and classify the result.
///
/// A zero exit that does not carry the expected `azlin-result:` marker is
/// treated as a failure. Reporting success for a step that silently did nothing
/// is exactly the bug class this guards against.
pub(crate) fn run_install_with_runner<F>(
    plan: &GuiInstallPlan,
    timeout_secs: u64,
    mut runner: F,
) -> Result<InstallOutcome>
where
    F: FnMut(&str, u64) -> Result<(i32, String, String)>,
{
    let script = wrap_for_shell(&build_install_script(plan));

    match runner(&script, timeout_secs) {
        Ok((0, stdout, _)) => parse_install_success(&stdout),
        Ok((code, _, stderr)) => {
            anyhow::bail!(
                "{}",
                describe_install_failure(code, &azlin_core::sanitizer::sanitize(&stderr))
            )
        }
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("timed out") {
                anyhow::bail!(
                    "GUI install timed out after {} minutes. The image pull is usually the slow \
                     step; check the VM's outbound network access and retry.",
                    timeout_secs / 60
                );
            }
            anyhow::bail!(
                "GUI install failed: {}",
                azlin_core::sanitizer::sanitize(&msg)
            );
        }
    }
}

/// Interpret the install script's stdout marker.
pub(crate) fn parse_install_success(stdout: &str) -> Result<InstallOutcome> {
    for line in stdout.lines() {
        match line.trim() {
            "azlin-result: already-installed" => return Ok(InstallOutcome::AlreadyInstalled),
            "azlin-result: installed" => return Ok(InstallOutcome::Installed),
            _ => {}
        }
    }
    anyhow::bail!(
        "GUI install reported success but produced no completion marker, so the desktop may not \
         actually be installed. Re-run with --verbose, or check `docker ps -a` on the VM."
    )
}

fn run_uninstall(target: &VmSshTarget, key_override: Option<&std::path::Path>) -> Result<()> {
    let script = wrap_for_shell(&build_uninstall_script());
    let pb = penguin_spinner("Removing remote desktop container...");
    let result = crate::dispatch_helpers::run_target_command_with_timeout(
        target,
        &script,
        GUI_UNINSTALL_TIMEOUT_SECS,
        key_override,
    );
    pb.finish_and_clear();

    match result {
        Ok((0, _, _)) => {
            println!("Remote desktop removed.");
            Ok(())
        }
        Ok((code, _, stderr)) => anyhow::bail!(
            "Failed to remove the remote desktop (exit {}): {}",
            code,
            azlin_core::sanitizer::sanitize(stderr.trim())
        ),
        Err(err) => anyhow::bail!(
            "Failed to remove the remote desktop: {}",
            azlin_core::sanitizer::sanitize(&err.to_string())
        ),
    }
}

/// Wrap a generated script so it runs under a login shell on the VM.
pub(crate) fn wrap_for_shell(script: &str) -> String {
    format!("bash -lc {}", crate::shell_escape(script))
}

#[cfg(test)]
mod tests {
    use super::*;
    use azlin_core::gui_container::GuiProtocol;

    fn plan() -> GuiInstallPlan {
        GuiInstallPlan::new(GuiProtocol::Vnc, DesktopGeometry::default())
    }

    #[test]
    fn a_created_container_reports_installed() {
        let outcome = run_install_with_runner(&plan(), 60, |_, _| {
            Ok((0, "azlin-result: installed\n".to_string(), String::new()))
        })
        .unwrap();
        assert_eq!(outcome, InstallOutcome::Installed);
    }

    #[test]
    fn a_matching_container_reports_already_installed() {
        let outcome = run_install_with_runner(&plan(), 60, |_, _| {
            Ok((
                0,
                "azlin-result: already-installed\n".to_string(),
                String::new(),
            ))
        })
        .unwrap();
        assert_eq!(outcome, InstallOutcome::AlreadyInstalled);
    }

    #[test]
    fn a_silent_success_is_treated_as_a_failure() {
        // A script that exits 0 without doing anything must not be reported as a
        // successful install.
        let err = run_install_with_runner(&plan(), 60, |_, _| Ok((0, String::new(), String::new())))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no completion marker"), "got: {err}");
    }

    #[test]
    fn docker_missing_produces_the_install_docker_remedy() {
        let err = run_install_with_runner(&plan(), 60, |_, _| {
            Ok((
                2,
                String::new(),
                "azlin-error: docker is not installed on this VM\n".to_string(),
            ))
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("docker is not installed"));
        assert!(err.contains("moby-engine"));
    }

    #[test]
    fn permission_failure_points_at_the_docker_group() {
        let err = run_install_with_runner(&plan(), 60, |_, _| {
            Ok((
                3,
                String::new(),
                "azlin-error: the docker daemon is not reachable as this user\n".to_string(),
            ))
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("usermod -aG docker"));
    }

    #[test]
    fn pull_failure_is_surfaced_not_swallowed() {
        let err = run_install_with_runner(&plan(), 60, |_, _| {
            Ok((
                4,
                String::new(),
                "azlin-error: failed to pull the desktop container image: no route to host\n"
                    .to_string(),
            ))
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("failed to pull"));
        assert!(err.contains("outbound network access"));
    }

    #[test]
    fn disk_exhaustion_is_reported_distinctly() {
        let err = run_install_with_runner(&plan(), 60, |_, _| {
            Ok((
                7,
                String::new(),
                "azlin-error: less than 4 GiB free for the container image\n".to_string(),
            ))
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("Free disk space"));
    }

    #[test]
    fn a_timeout_explains_the_likely_cause() {
        let err = run_install_with_runner(&plan(), 1_200, |_, _| {
            Err(anyhow::anyhow!("command timed out"))
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("timed out after 20 minutes"), "got: {err}");
    }

    #[test]
    fn the_script_is_passed_through_a_login_shell() {
        let mut seen = String::new();
        let _ = run_install_with_runner(&plan(), 60, |script, _| {
            seen = script.to_string();
            Ok((0, "azlin-result: installed".to_string(), String::new()))
        });
        assert!(seen.starts_with("bash -lc "));
    }

    #[test]
    fn the_configured_timeout_reaches_the_runner() {
        let mut seen = 0;
        let _ = run_install_with_runner(&plan(), 999, |_, timeout| {
            seen = timeout;
            Ok((0, "azlin-result: installed".to_string(), String::new()))
        });
        assert_eq!(seen, 999);
    }

    #[test]
    fn install_never_emits_an_azure_networking_command() {
        let mut seen = String::new();
        let _ = run_install_with_runner(&plan(), 60, |script, _| {
            seen = script.to_string();
            Ok((0, "azlin-result: installed".to_string(), String::new()))
        });
        let lowered = seen.to_ascii_lowercase();
        for forbidden in ["nsg", "az network", "network-security"] {
            assert!(
                !lowered.contains(forbidden),
                "install must never touch Azure networking ({forbidden})"
            );
        }
    }
}
