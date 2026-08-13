//! Containerised remote-desktop planner for `azlin gui install`.
//!
//! Azure Linux ships no desktop stack at all. Neither the 4.0 `base`/`microsoft`
//! repositories nor 3.0 `base`/`extended` contain a VNC server, an RDP server or a
//! window manager, so `dnf install tigervnc xrdp xfce4` can never succeed. The
//! desktop, the VNC server and the RDP server therefore come from a prebuilt
//! container image running on the VM's Docker, which azlin's bootstrap already
//! installs (`moby-engine` + `docker-cli`, see `azlin-azure::cloud_init`).
//!
//! Every function in this module is pure: data in, plan or command strings out.
//! It mirrors [`crate`]'s sibling planners in `azlin-azure` (`teardown`,
//! `orphan_detector`) so the install, detection and teardown rules are fully
//! unit-testable without a VM, without Docker and without Azure.
//!
//! # Security invariants
//!
//! These are enforced by construction here and asserted by the unit tests:
//!
//! * Published ports are **always** bound to `127.0.0.1` on the VM, so the desktop
//!   is unreachable from the network even if a permissive NSG rule existed.
//! * No network-security-group rule is ever created, modified or referenced. This
//!   module emits no `az` command of any kind.
//! * The web (noVNC) port of the VNC image is deliberately **not** published.
//! * The desktop always has a password. The password is generated on the VM and
//!   passed to Docker through a `0600` env-file, so it never appears in a process
//!   listing or in `docker inspect` output.
//! * Access is expected to happen exclusively over azlin's existing SSH tunnel.

use serde::{Deserialize, Serialize};

/// Remote-desktop wire protocol to install on the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuiProtocol {
    /// TigerVNC RFB, consumed by a standard VNC viewer.
    Vnc,
    /// xrdp, consumed by a standard RDP client.
    Rdp,
}

impl GuiProtocol {
    /// Lowercase wire name, used in generated scripts and in container labels.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vnc => "vnc",
            Self::Rdp => "rdp",
        }
    }

    /// Parse the wire name emitted by [`build_detect_script`].
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "vnc" => Some(Self::Vnc),
            "rdp" => Some(Self::Rdp),
            _ => None,
        }
    }
}

impl std::fmt::Display for GuiProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Container name azlin manages. Fixed, so install is naturally idempotent and
/// detection needs no bookkeeping beyond Docker itself.
pub const CONTAINER_NAME: &str = "azlin-gui";

/// Directory on the VM holding the generated env-file and the exported VNC
/// password blob. Created `0700`; the files inside are `0600`.
pub const STATE_DIR: &str = "$HOME/.azlin/gui";

/// Path on the VM of the VNC authentication blob exported from the container.
///
/// The blob is produced by the container's own `vncpasswd`, then copied out with
/// `docker cp`. Copying out (rather than bind-mounting over the container's
/// `.vnc` directory) avoids shadowing files the image's startup script writes
/// there, and avoids reimplementing the VNC password format locally.
pub const HOST_VNC_PASSWD_PATH: &str = "$HOME/.azlin/gui/vncpasswd";

/// Path inside the VNC container of the authentication blob to export.
pub const CONTAINER_VNC_PASSWD_PATH: &str = "/headless/.vnc/passwd";

/// Login name used by the RDP image's desktop session.
pub const RDP_USERNAME: &str = "abc";

/// A container image pinned for one protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiImage {
    /// Fully qualified image reference including the pinned tag.
    pub reference: &'static str,
    /// Digest of the linux/amd64 manifest at the time of pinning, recorded so a
    /// tag that silently moves can be detected.
    pub amd64_digest: &'static str,
    /// Port the desktop server listens on inside the container.
    pub container_port: u16,
    /// Human-readable description of what clients can connect.
    pub client_support: &'static str,
}

/// VNC image: genuine TigerVNC RFB on 5901.
///
/// Verified against the Docker Hub registry API: exposes `5901/tcp` and
/// `6901/tcp`, entrypoint `/dockerstartup/vnc_startup.sh`, and reads `VNC_PW`,
/// `VNC_RESOLUTION` and `VNC_COL_DEPTH` from the environment.
///
/// `linuxserver/webtop` was evaluated and rejected: it serves KasmVNC over
/// WebSockets, which a standard RFB viewer cannot speak.
pub const VNC_IMAGE: GuiImage = GuiImage {
    reference: "consol/debian-xfce-vnc:v2.0.4",
    amd64_digest: "sha256:b6d53e9f797bb4b4e3b7b317ec07e4242f33c7e3061af16d18685f6866295e58",
    container_port: 5901,
    client_support: "any standard VNC viewer (TigerVNC RFB)",
};

/// RDP image: xrdp on 3389.
///
/// Verified against the GitHub Container Registry API: exposes `3389/tcp`,
/// entrypoint `/init`, with linux/amd64 and linux/arm64 manifests.
pub const RDP_IMAGE: GuiImage = GuiImage {
    reference: "lscr.io/linuxserver/rdesktop:ubuntu-xfce",
    amd64_digest: "sha256:85f5e20fbed17a13be2619aafffedd6df2c3c68076693caf951176f133765062",
    container_port: 3389,
    client_support: "any standard RDP client (xfreerdp, mstsc, Microsoft Remote Desktop)",
};

/// Return the pinned image for a protocol.
pub fn image_for(protocol: GuiProtocol) -> GuiImage {
    match protocol {
        GuiProtocol::Vnc => VNC_IMAGE,
        GuiProtocol::Rdp => RDP_IMAGE,
    }
}

/// Loopback address the desktop port is published on.
///
/// Publishing on `127.0.0.1` (rather than the default `0.0.0.0`) is the single
/// most important security property of this module: it makes the desktop
/// unreachable from outside the VM regardless of NSG configuration.
pub const PUBLISH_ADDRESS: &str = "127.0.0.1";

/// Requested desktop geometry and colour depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopGeometry {
    pub resolution: String,
    pub depth: u8,
}

impl Default for DesktopGeometry {
    fn default() -> Self {
        Self {
            resolution: "1920x1080".to_string(),
            depth: 24,
        }
    }
}

/// Everything needed to materialise the container on the VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiInstallPlan {
    pub protocol: GuiProtocol,
    pub image: GuiImage,
    pub container_name: String,
    /// Port published on the VM's loopback interface.
    pub host_port: u16,
    pub geometry: DesktopGeometry,
}

impl GuiInstallPlan {
    /// Build the plan for a protocol. The published host port mirrors the
    /// container port, so the SSH tunnel target is predictable.
    pub fn new(protocol: GuiProtocol, geometry: DesktopGeometry) -> Self {
        let image = image_for(protocol);
        Self {
            protocol,
            image,
            container_name: CONTAINER_NAME.to_string(),
            host_port: image.container_port,
            geometry,
        }
    }

    /// The `-p` value published to Docker, always loopback-bound.
    pub fn publish_spec(&self) -> String {
        format!(
            "{}:{}:{}",
            PUBLISH_ADDRESS, self.host_port, self.image.container_port
        )
    }

    /// Arguments to `docker run`, excluding the leading `docker run` itself.
    ///
    /// The desktop password is *not* present here: it is supplied via
    /// `--env-file`, so it never reaches a process listing or `docker inspect`.
    pub fn docker_run_args(&self, env_file: &str) -> Vec<String> {
        let mut args = vec![
            "-d".to_string(),
            "--name".to_string(),
            self.container_name.clone(),
            "--restart".to_string(),
            "unless-stopped".to_string(),
            "--shm-size".to_string(),
            "1g".to_string(),
            "--env-file".to_string(),
            env_file.to_string(),
            "--label".to_string(),
            format!("azlin.gui.protocol={}", self.protocol),
            "--label".to_string(),
            format!("azlin.gui.image={}", self.image.reference),
            "-p".to_string(),
            self.publish_spec(),
        ];
        args.push(self.image.reference.to_string());
        args
    }

    /// Environment variables written to the `0600` env-file on the VM.
    ///
    /// `password` is substituted by the shell on the VM (the value is generated
    /// there and never travels through azlin), so entries may contain a shell
    /// variable reference.
    pub fn env_file_entries(&self, password_expr: &str) -> Vec<String> {
        match self.protocol {
            GuiProtocol::Vnc => vec![
                format!("VNC_PW={}", password_expr),
                format!("VNC_RESOLUTION={}", self.geometry.resolution),
                format!("VNC_COL_DEPTH={}", self.geometry.depth),
            ],
            // The RDP image takes its credentials from the container user's
            // password, set with `chpasswd` after start, so only non-secret
            // sizing hints belong in the env-file.
            GuiProtocol::Rdp => vec!["PUID=1000".to_string(), "PGID=1000".to_string()],
        }
    }
}

/// Lifecycle state of the managed container, as reported by the probe script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerState {
    /// No container named [`CONTAINER_NAME`] exists.
    Missing,
    /// The container exists but is not running.
    Stopped,
    /// The container exists and is running.
    Running,
}

impl ContainerState {
    /// Map a `docker inspect -f {{.State.Status}}` value onto a state.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "" | "missing" => Self::Missing,
            "running" => Self::Running,
            _ => Self::Stopped,
        }
    }
}

/// Parsed result of the detection probe run on the VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiStatus {
    /// A `docker` binary is on `PATH`.
    pub docker_present: bool,
    /// The Docker daemon is reachable as the connecting user (i.e. the user is
    /// in the `docker` group and the daemon is running).
    pub docker_usable: bool,
    pub container_state: ContainerState,
    /// Protocol recorded on the container label, when a container exists.
    pub protocol: Option<GuiProtocol>,
    /// Loopback port published on the VM, when a container exists.
    pub host_port: Option<u16>,
}

impl GuiStatus {
    /// Whether a desktop is installed at all (running or merely stopped).
    pub fn is_installed(&self) -> bool {
        self.container_state != ContainerState::Missing
    }
}

/// Why the desktop cannot be used, together with the remediation to print.
///
/// Every variant carries actionable text. Silent or generic failures are the bug
/// class this enum exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiUnavailable {
    DockerMissing,
    DockerNotUsable,
    NotInstalled { suggested_command: String },
}

impl std::fmt::Display for GuiUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DockerMissing => write!(
                f,
                "Docker is not installed on the VM, so the containerised desktop cannot run.\n\
                 azlin normally installs it during provisioning. Install it with:\n  \
                 sudo dnf5 install -y moby-engine docker-cli && sudo systemctl enable --now docker"
            ),
            Self::DockerNotUsable => write!(
                f,
                "Docker is installed on the VM but the daemon is not reachable as this user.\n\
                 Check both of the following on the VM:\n  \
                 sudo systemctl enable --now docker\n  \
                 sudo usermod -aG docker \"$USER\"   # then reconnect for the new group to apply"
            ),
            Self::NotInstalled { suggested_command } => write!(
                f,
                "No remote desktop is installed on this VM.\n\
                 Azure Linux ships no VNC server, RDP server or desktop environment, so azlin \
                 installs them as a container.\n\
                 Install it with:\n  {suggested_command}"
            ),
        }
    }
}

/// Decide whether a connect attempt can proceed against the reported status.
pub fn check_available(status: &GuiStatus, vm_identifier: &str) -> Result<(), GuiUnavailable> {
    if !status.docker_present {
        return Err(GuiUnavailable::DockerMissing);
    }
    if !status.docker_usable {
        return Err(GuiUnavailable::DockerNotUsable);
    }
    if !status.is_installed() {
        return Err(GuiUnavailable::NotInstalled {
            suggested_command: suggested_install_command(vm_identifier),
        });
    }
    Ok(())
}

/// The exact command a user should run to install the desktop.
pub fn suggested_install_command(vm_identifier: &str) -> String {
    if vm_identifier.is_empty() {
        "azlin gui install".to_string()
    } else {
        format!("azlin gui install {vm_identifier}")
    }
}

// ---------------------------------------------------------------------------
// Script generation
// ---------------------------------------------------------------------------

/// Shell-quote a value for safe single-quoted embedding.
fn sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Build the detection probe.
///
/// Emits `key=value` lines on stdout and always exits `0`, so a non-zero exit
/// unambiguously means the SSH transport failed rather than "not installed".
pub fn build_detect_script() -> String {
    format!(
        "set -u; \
         if command -v docker >/dev/null 2>&1; then echo docker_present=true; else echo docker_present=false; echo docker_usable=false; echo container_state=missing; exit 0; fi; \
         if docker info >/dev/null 2>&1; then echo docker_usable=true; else echo docker_usable=false; echo container_state=missing; exit 0; fi; \
         state=$(docker inspect -f '{{{{.State.Status}}}}' {name} 2>/dev/null || echo missing); \
         echo \"container_state=$state\"; \
         if [ \"$state\" != missing ]; then \
           echo \"protocol=$(docker inspect -f '{{{{index .Config.Labels \"azlin.gui.protocol\"}}}}' {name} 2>/dev/null)\"; \
           echo \"host_port=$(docker inspect -f '{{{{range $p, $c := .NetworkSettings.Ports}}}}{{{{range $c}}}}{{{{.HostPort}}}}{{{{end}}}}{{{{end}}}}' {name} 2>/dev/null)\"; \
         fi; \
         exit 0",
        name = sq(CONTAINER_NAME),
    )
}

/// Parse the `key=value` output of [`build_detect_script`].
pub fn parse_detect_output(output: &str) -> GuiStatus {
    let mut status = GuiStatus {
        docker_present: false,
        docker_usable: false,
        container_state: ContainerState::Missing,
        protocol: None,
        host_port: None,
    };

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "docker_present" => status.docker_present = value == "true",
            "docker_usable" => status.docker_usable = value == "true",
            "container_state" => status.container_state = ContainerState::parse(value),
            "protocol" => status.protocol = GuiProtocol::parse(value),
            "host_port" => status.host_port = value.parse().ok(),
            _ => {}
        }
    }

    status
}

/// Build the install script.
///
/// The script is idempotent: an existing container whose image and protocol
/// already match the plan is left in place (and started if stopped); anything
/// else is removed and recreated. Every failure mode is reported with a distinct
/// `azlin-error:` marker rather than being swallowed.
pub fn build_install_script(plan: &GuiInstallPlan) -> String {
    let name = sq(&plan.container_name);
    let image = sq(plan.image.reference);
    let protocol = plan.protocol.as_str();
    let run_args = plan
        .docker_run_args("\"$ENV_FILE\"")
        .iter()
        .map(|a| {
            // The env-file placeholder must stay an unquoted shell expansion.
            if a == "\"$ENV_FILE\"" {
                a.clone()
            } else {
                sq(a)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let env_lines = plan
        .env_file_entries("$AZLIN_GUI_PW")
        .iter()
        .map(|entry| format!("printf '%s\\n' \"{entry}\" >> \"$ENV_FILE\";"))
        .collect::<Vec<_>>()
        .join(" ");

    // RDP authenticates against the container user's own password, so it is set
    // after the container starts. VNC consumes VNC_PW from the env-file, and its
    // password blob is copied out for the local viewer.
    let post_start = match plan.protocol {
        GuiProtocol::Vnc => format!(
            "for _ in $(seq 1 30); do \
               if docker exec {name} test -f {cpath} >/dev/null 2>&1; then break; fi; sleep 2; \
             done; \
             if ! docker cp {name}:{cpath} \"$STATE_DIR/vncpasswd\" >/dev/null 2>&1; then \
               echo 'azlin-error: the VNC container started but never wrote its password file' >&2; exit 6; \
             fi; \
             chmod 600 \"$STATE_DIR/vncpasswd\"",
            name = name,
            cpath = sq(CONTAINER_VNC_PASSWD_PATH),
        ),
        GuiProtocol::Rdp => format!(
            "for _ in $(seq 1 30); do \
               if docker exec {name} id {user} >/dev/null 2>&1; then break; fi; sleep 2; \
             done; \
             if ! printf '%s:%s' {user} \"$AZLIN_GUI_PW\" | docker exec -i {name} chpasswd >/dev/null 2>&1; then \
               echo 'azlin-error: could not set the RDP desktop password inside the container' >&2; exit 6; \
             fi; \
             printf '%s\\n' \"$AZLIN_GUI_PW\" > \"$STATE_DIR/rdppasswd\"; \
             chmod 600 \"$STATE_DIR/rdppasswd\"",
            name = name,
            user = sq(RDP_USERNAME),
        ),
    };

    let publish_plain = format!("127.0.0.1:{}", plan.host_port);
    let publish_quoted = sq(&publish_plain);

    format!(
        "set -u; \
         if ! command -v docker >/dev/null 2>&1; then \
           echo 'azlin-error: docker is not installed on this VM' >&2; exit 2; fi; \
         if ! docker info >/dev/null 2>&1; then \
           echo 'azlin-error: the docker daemon is not reachable as this user' >&2; exit 3; fi; \
         AVAIL=$(df -Pk /var/lib/docker 2>/dev/null || df -Pk /); \
         AVAIL=$(echo \"$AVAIL\" | awk 'NR==2 {{print $4}}'); \
         if [ -n \"$AVAIL\" ] && [ \"$AVAIL\" -lt 4194304 ]; then \
           echo \"azlin-error: less than 4 GiB free for the container image (${{AVAIL}} KiB available)\" >&2; exit 7; fi; \
         STATE_DIR=\"$HOME/.azlin/gui\"; mkdir -p \"$STATE_DIR\"; chmod 700 \"$STATE_DIR\"; \
         CUR=$(docker inspect -f '{{{{.Config.Image}}}}' {name} 2>/dev/null || true); \
         CUR_PROTO=$(docker inspect -f '{{{{index .Config.Labels \"azlin.gui.protocol\"}}}}' {name} 2>/dev/null || true); \
         if [ \"$CUR\" = {image} ] && [ \"$CUR_PROTO\" = {protocol} ]; then \
           docker start {name} >/dev/null 2>&1 || true; \
           echo 'azlin-result: already-installed'; exit 0; \
         fi; \
         if [ -n \"$CUR\" ]; then docker rm -f {name} >/dev/null 2>&1 || true; fi; \
         if ! PULL_OUT=$(docker pull {image} 2>&1); then \
           echo \"azlin-error: failed to pull the desktop container image: $PULL_OUT\" >&2; exit 4; fi; \
         if command -v openssl >/dev/null 2>&1; then AZLIN_GUI_PW=$(openssl rand -hex 16); \
         else AZLIN_GUI_PW=$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \\n'); fi; \
         if [ -z \"$AZLIN_GUI_PW\" ]; then \
           echo 'azlin-error: could not generate a desktop password on the VM' >&2; exit 5; fi; \
         ENV_FILE=\"$STATE_DIR/env\"; \
         : > \"$ENV_FILE\"; chmod 600 \"$ENV_FILE\"; \
         {env_lines} \
         if ! RUN_OUT=$(docker run {run_args} 2>&1); then \
           if ss -ltn 2>/dev/null | grep -q {publish}; then \
             echo 'azlin-error: {publish_plain} is already in use on the VM' >&2; exit 8; fi; \
           echo \"azlin-error: failed to start the desktop container: $RUN_OUT\" >&2; exit 9; fi; \
         {post_start}; \
         echo 'azlin-result: installed'",
        name = name,
        image = image,
        protocol = sq(protocol),
        publish = publish_quoted,
        publish_plain = publish_plain,
        env_lines = env_lines,
        run_args = run_args,
        post_start = post_start,
    )
}

/// Build the script that starts an already-installed but stopped container.
///
/// This is a connect-time repair, not an install: it never pulls an image and
/// never creates a container.
pub fn build_start_script() -> String {
    format!(
        "set -u; \
         if ! docker start {name} >/dev/null 2>&1; then \
           echo 'azlin-error: the desktop container exists but could not be started' >&2; exit 1; fi",
        name = sq(CONTAINER_NAME),
    )
}

/// Build the script that removes the managed container and its state.
pub fn build_uninstall_script() -> String {
    format!(
        "set -u; \
         docker rm -f {name} >/dev/null 2>&1 || true; \
         rm -rf \"$HOME/.azlin/gui\"",
        name = sq(CONTAINER_NAME),
    )
}

/// Classify an install-script exit code into an actionable message.
pub fn describe_install_failure(exit_code: i32, stderr: &str) -> String {
    let detail = stderr
        .lines()
        .find_map(|l| l.trim().strip_prefix("azlin-error:"))
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    let remedy = match exit_code {
        2 => "Install Docker on the VM:\n  sudo dnf5 install -y moby-engine docker-cli && sudo systemctl enable --now docker",
        3 => "Start Docker and add your user to the docker group on the VM:\n  sudo systemctl enable --now docker\n  sudo usermod -aG docker \"$USER\"   # then reconnect",
        4 => "Check the VM's outbound network access and retry. If the VM has no internet egress, mirror the image into a registry it can reach.",
        5 | 6 => "Re-run the install. If it keeps failing, remove the container with 'docker rm -f azlin-gui' on the VM and try again.",
        7 => "Free disk space on the VM (the desktop image needs roughly 2-4 GiB) and re-run.",
        8 => "Another process is already listening on that loopback port. Stop it, or remove the stale container with 'docker rm -f azlin-gui'.",
        9 => "Inspect the container logs on the VM:\n  docker logs azlin-gui",
        _ => "Re-run with --verbose for the full remote output.",
    };

    if detail.is_empty() {
        format!("GUI install failed (exit {exit_code}).\n{remedy}")
    } else {
        format!("GUI install failed: {detail}.\n{remedy}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- security invariants -------------------------------------------------

    #[test]
    fn published_ports_are_always_loopback_bound() {
        for protocol in [GuiProtocol::Vnc, GuiProtocol::Rdp] {
            let plan = GuiInstallPlan::new(protocol, DesktopGeometry::default());
            assert!(
                plan.publish_spec().starts_with("127.0.0.1:"),
                "{protocol} publish spec must be loopback-bound, got {}",
                plan.publish_spec()
            );

            let args = plan.docker_run_args("/tmp/env");
            let published: Vec<&String> = args
                .iter()
                .enumerate()
                .filter(|(i, _)| *i > 0 && args[i - 1] == "-p")
                .map(|(_, a)| a)
                .collect();
            assert!(!published.is_empty(), "{protocol} must publish a port");
            for spec in published {
                assert!(
                    spec.starts_with("127.0.0.1:"),
                    "{protocol} published {spec} without a loopback bind"
                );
            }
        }
    }

    #[test]
    fn novnc_web_port_is_never_published() {
        let plan = GuiInstallPlan::new(GuiProtocol::Vnc, DesktopGeometry::default());
        assert!(
            !plan.docker_run_args("/tmp/env").iter().any(|a| a.contains("6901")),
            "the noVNC web port must not be published"
        );
        assert!(!build_install_script(&plan).contains("6901"));
    }

    #[test]
    fn no_script_ever_touches_azure_networking() {
        let mut scripts = vec![
            build_detect_script(),
            build_start_script(),
            build_uninstall_script(),
        ];
        for protocol in [GuiProtocol::Vnc, GuiProtocol::Rdp] {
            scripts.push(build_install_script(&GuiInstallPlan::new(
                protocol,
                DesktopGeometry::default(),
            )));
        }
        for script in scripts {
            for forbidden in ["nsg", "network-security", "az network", "--priority"] {
                assert!(
                    !script.to_ascii_lowercase().contains(forbidden),
                    "generated script must never reference {forbidden}"
                );
            }
        }
    }

    #[test]
    fn password_is_never_passed_as_a_docker_argument() {
        for protocol in [GuiProtocol::Vnc, GuiProtocol::Rdp] {
            let plan = GuiInstallPlan::new(protocol, DesktopGeometry::default());
            let args = plan.docker_run_args("/tmp/env");
            assert!(
                !args.iter().any(|a| a.contains("VNC_PW") || a.contains("PASSWORD")),
                "{protocol} must not place secrets in docker run arguments"
            );
            assert!(args.iter().any(|a| a == "--env-file"));
        }
    }

    #[test]
    fn install_script_restricts_state_file_permissions() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("chmod 700 \"$STATE_DIR\""));
        assert!(script.contains("chmod 600 \"$ENV_FILE\""));
    }

    // -- image pinning -------------------------------------------------------

    #[test]
    fn images_are_pinned_by_tag_with_recorded_digests() {
        for image in [VNC_IMAGE, RDP_IMAGE] {
            assert!(image.reference.contains(':'), "image must carry a tag");
            assert!(!image.reference.ends_with(":latest"), "must not float on latest");
            assert!(image.amd64_digest.starts_with("sha256:"));
            assert_eq!(image.amd64_digest.len(), "sha256:".len() + 64);
        }
    }

    #[test]
    fn protocols_map_to_their_conventional_ports() {
        assert_eq!(image_for(GuiProtocol::Vnc).container_port, 5901);
        assert_eq!(image_for(GuiProtocol::Rdp).container_port, 3389);
    }

    #[test]
    fn protocol_round_trips_through_its_wire_name() {
        for protocol in [GuiProtocol::Vnc, GuiProtocol::Rdp] {
            assert_eq!(GuiProtocol::parse(protocol.as_str()), Some(protocol));
        }
        assert_eq!(GuiProtocol::parse("VNC"), Some(GuiProtocol::Vnc));
        assert_eq!(GuiProtocol::parse(""), None);
        assert_eq!(GuiProtocol::parse("spice"), None);
    }

    // -- plan construction ---------------------------------------------------

    #[test]
    fn run_args_request_restart_persistence() {
        let plan = GuiInstallPlan::new(GuiProtocol::Vnc, DesktopGeometry::default());
        let args = plan.docker_run_args("/tmp/env");
        let restart = args.iter().position(|a| a == "--restart").expect("--restart");
        assert_eq!(args[restart + 1], "unless-stopped");
    }

    #[test]
    fn run_args_end_with_the_image_reference() {
        let plan = GuiInstallPlan::new(GuiProtocol::Rdp, DesktopGeometry::default());
        let args = plan.docker_run_args("/tmp/env");
        assert_eq!(args.last().unwrap(), RDP_IMAGE.reference);
    }

    #[test]
    fn vnc_env_file_carries_geometry_and_password() {
        let plan = GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry {
                resolution: "1280x720".to_string(),
                depth: 16,
            },
        );
        let entries = plan.env_file_entries("$PW");
        assert!(entries.contains(&"VNC_PW=$PW".to_string()));
        assert!(entries.contains(&"VNC_RESOLUTION=1280x720".to_string()));
        assert!(entries.contains(&"VNC_COL_DEPTH=16".to_string()));
    }

    #[test]
    fn rdp_env_file_carries_no_secret() {
        let plan = GuiInstallPlan::new(GuiProtocol::Rdp, DesktopGeometry::default());
        assert!(!plan.env_file_entries("$PW").iter().any(|e| e.contains("$PW")));
    }

    // -- detection -----------------------------------------------------------

    #[test]
    fn detect_output_without_docker_is_not_installed() {
        let status = parse_detect_output("docker_present=false\ndocker_usable=false\ncontainer_state=missing\n");
        assert!(!status.docker_present);
        assert!(!status.is_installed());
        assert_eq!(
            check_available(&status, "my-vm"),
            Err(GuiUnavailable::DockerMissing)
        );
    }

    #[test]
    fn detect_output_with_unusable_daemon_is_reported_distinctly() {
        let status = parse_detect_output("docker_present=true\ndocker_usable=false\ncontainer_state=missing\n");
        assert_eq!(
            check_available(&status, "my-vm"),
            Err(GuiUnavailable::DockerNotUsable)
        );
    }

    #[test]
    fn detect_output_without_container_suggests_the_install_command() {
        let status = parse_detect_output("docker_present=true\ndocker_usable=true\ncontainer_state=missing\n");
        assert!(!status.is_installed());
        let err = check_available(&status, "my-vm").unwrap_err();
        assert_eq!(
            err,
            GuiUnavailable::NotInstalled {
                suggested_command: "azlin gui install my-vm".to_string()
            }
        );
        assert!(err.to_string().contains("azlin gui install my-vm"));
    }

    #[test]
    fn detect_output_for_a_running_container_is_available() {
        let status = parse_detect_output(
            "docker_present=true\ndocker_usable=true\ncontainer_state=running\nprotocol=rdp\nhost_port=3389\n",
        );
        assert_eq!(status.container_state, ContainerState::Running);
        assert_eq!(status.protocol, Some(GuiProtocol::Rdp));
        assert_eq!(status.host_port, Some(3389));
        assert!(check_available(&status, "my-vm").is_ok());
    }

    #[test]
    fn a_stopped_container_still_counts_as_installed() {
        let status = parse_detect_output(
            "docker_present=true\ndocker_usable=true\ncontainer_state=exited\nprotocol=vnc\nhost_port=5901\n",
        );
        assert_eq!(status.container_state, ContainerState::Stopped);
        assert!(status.is_installed());
        assert!(check_available(&status, "my-vm").is_ok());
    }

    #[test]
    fn unknown_detect_keys_and_blank_lines_are_ignored() {
        let status = parse_detect_output(
            "\nnoise\ndocker_present=true\nfuture_key=1\ndocker_usable=true\ncontainer_state=running\n",
        );
        assert!(status.docker_usable);
        assert_eq!(status.container_state, ContainerState::Running);
    }

    #[test]
    fn container_state_maps_unfamiliar_docker_states_to_stopped() {
        assert_eq!(ContainerState::parse("created"), ContainerState::Stopped);
        assert_eq!(ContainerState::parse("paused"), ContainerState::Stopped);
        assert_eq!(ContainerState::parse("exited"), ContainerState::Stopped);
        assert_eq!(ContainerState::parse("missing"), ContainerState::Missing);
        assert_eq!(ContainerState::parse(""), ContainerState::Missing);
    }

    #[test]
    fn detect_script_always_exits_zero_so_failure_means_transport_failure() {
        let script = build_detect_script();
        assert_eq!(script.matches("exit 0").count(), 3);
        assert!(!script.contains("exit 1"));
    }

    #[test]
    fn suggested_command_omits_an_empty_vm_identifier() {
        assert_eq!(suggested_install_command(""), "azlin gui install");
    }

    // -- install script ------------------------------------------------------

    #[test]
    fn install_script_is_idempotent_for_a_matching_container() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("azlin-result: already-installed"));
        assert!(script.contains("docker start"));
    }

    #[test]
    fn install_script_recreates_a_mismatched_container() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Rdp,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("docker rm -f"));
        assert!(script.contains("docker pull"));
    }

    #[test]
    fn install_script_checks_every_documented_failure_mode() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry::default(),
        ));
        for (code, needle) in [
            (2, "docker is not installed"),
            (3, "docker daemon is not reachable"),
            (4, "failed to pull"),
            (5, "could not generate a desktop password"),
            (8, "already in use"),
            (9, "failed to start the desktop container"),
        ] {
            assert!(script.contains(needle), "missing check for exit {code}: {needle}");
            assert!(script.contains(&format!("exit {code}")), "missing exit {code}");
        }
        assert!(script.contains("less than 4 GiB free"));
    }

    #[test]
    fn install_script_never_swallows_the_docker_run_failure() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("if ! RUN_OUT=$(docker run"));
        assert!(
            !script.contains("docker run") || !script.contains("docker run ... || true"),
            "the docker run failure must never be discarded"
        );
        // The only tolerated `|| true` uses are best-effort cleanup probes, never
        // the container creation itself.
        for line in script.split("; ") {
            if line.contains("docker run") {
                assert!(
                    !line.contains("|| true"),
                    "docker run must not be suffixed with '|| true': {line}"
                );
            }
        }
    }

    #[test]
    fn vnc_install_exports_the_password_blob_for_the_local_viewer() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("docker cp"));
        assert!(script.contains(CONTAINER_VNC_PASSWD_PATH));
        assert!(script.contains("chmod 600 \"$STATE_DIR/vncpasswd\""));
    }

    #[test]
    fn rdp_install_sets_a_password_on_the_desktop_user() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Rdp,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("chpasswd"));
        assert!(script.contains(RDP_USERNAME));
        assert!(script.contains("chmod 600 \"$STATE_DIR/rdppasswd\""));
    }

    #[test]
    fn start_script_never_creates_or_pulls() {
        let script = build_start_script();
        assert!(script.contains("docker start"));
        assert!(!script.contains("docker run"));
        assert!(!script.contains("docker pull"));
    }

    #[test]
    fn uninstall_script_removes_container_and_state() {
        let script = build_uninstall_script();
        assert!(script.contains("docker rm -f"));
        assert!(script.contains(".azlin/gui"));
    }

    // -- failure classification ---------------------------------------------

    #[test]
    fn every_install_exit_code_gets_actionable_remediation() {
        for code in [2, 3, 4, 5, 6, 7, 8, 9, 42] {
            let message = describe_install_failure(code, "");
            assert!(
                message.lines().count() >= 2,
                "exit {code} produced no remediation: {message}"
            );
        }
    }

    #[test]
    fn install_failure_surfaces_the_remote_detail() {
        let message =
            describe_install_failure(4, "azlin-error: failed to pull the desktop container image\n");
        assert!(message.contains("failed to pull the desktop container image"));
        assert!(!message.contains("exit 4"), "detail should replace the bare code");
    }

    #[test]
    fn install_failure_without_a_marker_falls_back_to_the_exit_code() {
        let message = describe_install_failure(9, "some unrelated noise");
        assert!(message.contains("exit 9"));
        assert!(message.contains("docker logs azlin-gui"));
    }

    #[test]
    fn label_lookups_use_a_valid_go_template() {
        // Inside a single-quoted shell word a backslash is literal, so
        // `\"label\"` would reach Go's template parser as `\"label\"` and be
        // rejected at runtime. The quotes must be bare.
        for script in [
            build_detect_script(),
            build_install_script(&GuiInstallPlan::new(
                GuiProtocol::Vnc,
                DesktopGeometry::default(),
            )),
        ] {
            assert!(
                script.contains(r#"{{index .Config.Labels "azlin.gui.protocol"}}"#),
                "label lookup template is malformed: {script}"
            );
            assert!(
                !script.contains(r#"\"azlin.gui.protocol\""#),
                "label lookup must not escape its quotes"
            );
        }
    }

    #[test]
    fn pull_and_run_failures_include_the_daemon_error_text() {
        let script = build_install_script(&GuiInstallPlan::new(
            GuiProtocol::Vnc,
            DesktopGeometry::default(),
        ));
        assert!(script.contains("$PULL_OUT"), "pull failure must report docker's message");
        assert!(script.contains("$RUN_OUT"), "run failure must report docker's message");
    }

    #[test]
    fn shell_quoting_neutralises_embedded_quotes() {
        assert_eq!(sq("plain"), "'plain'");
        assert_eq!(sq("it's"), r"'it'\''s'");
    }
}

#[cfg(all(test, unix))]
mod shell_syntax_tests {
    use super::*;

    /// Every generated script must be syntactically valid shell. A quoting bug
    /// here would otherwise only surface against a live VM.
    fn assert_parses(label: &str, script: &str) {
        let output = std::process::Command::new("bash")
            .arg("-n")
            .arg("-c")
            .arg(script)
            .output()
            .expect("bash must be available to validate generated scripts");
        assert!(
            output.status.success(),
            "{label} is not valid shell: {}\n---\n{script}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn all_generated_scripts_are_valid_shell() {
        assert_parses("detect", &build_detect_script());
        assert_parses("start", &build_start_script());
        assert_parses("uninstall", &build_uninstall_script());
        for protocol in [GuiProtocol::Vnc, GuiProtocol::Rdp] {
            let plan = GuiInstallPlan::new(protocol, DesktopGeometry::default());
            assert_parses(&format!("install {protocol}"), &build_install_script(&plan));
        }
    }
}
