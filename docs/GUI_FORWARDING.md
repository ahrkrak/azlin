# GUI Forwarding & Remote Desktop

Run graphical applications on your Azure VMs and display them locally. azlin supports two approaches: a **containerised remote desktop** (VNC or RDP) for a full session, and **X11 forwarding** for lightweight GUI apps.

## Overview

| Approach | Best For | Latency | Setup |
|----------|----------|---------|-------|
| VNC desktop (container) | Full XFCE desktop, multiple apps | Higher (full desktop) | `azlin gui install` |
| RDP desktop (container) | Full XFCE desktop from a Windows/RDP client | Higher (full desktop) | `azlin gui install --protocol rdp` |
| X11 forwarding | Individual GUI apps (gitk, meld, xeyes) | Low (per-window) | Minimal |

Both approaches work transparently through Azure Bastion tunnels when your VM has no public IP.

## Prerequisites

### Local Machine

**WSL2 (Windows)**:
- WSLg is included with WSL2 by default and provides an X server automatically.
- Verify with: `echo $DISPLAY` (should show something like `:0`)

**Linux**:
- An X11 display server is already running if you are in a graphical session.
- Verify with: `echo $DISPLAY`

**macOS**:
- Install [XQuartz](https://www.xquartz.org/): `brew install --cask xquartz`
- Log out and back in after installation.
- Enable "Allow connections from network clients" in XQuartz Preferences > Security.

**VNC viewer** (for `azlin gui --protocol vnc`, the default):
- `azlin gui` launches a local `vncviewer` command.
- [TigerVNC](https://tigervnc.org/) is the tested viewer and provides that binary on Linux, macOS, and Windows/WSL setups.

**RDP client** (for `azlin gui` against an RDP install):
- azlin looks for `xfreerdp3`, `xfreerdp` or `mstsc` and launches it automatically.
- If none is found it prints manual connection details instead.

### Remote VM

The desktop stack is installed once with `azlin gui install`, which runs it as
a container on the VM's Docker (installed by the azlin bootstrap). `azlin gui`
itself never installs anything implicitly. `azlin connect --x11` does **not**
install remote GUI applications or X11 packages for you; it only enables X11
forwarding on the SSH connection.

## Remote Desktop (containerised)

Azure Linux ships **no** desktop environment, VNC server or RDP server in its
repositories — not in 4.0 (`base` + `microsoft`) and not in 3.0 (`base` +
`extended`). A package-based install can therefore never succeed. azlin instead
runs a prebuilt desktop container on the VM's Docker, which the azlin bootstrap
already installs (`moby-engine` + `docker-cli`).

Install the stack once, then connect as often as you like:

```bash
# Install the desktop stack (VNC, the default)
azlin gui install my-vm

# Or install the RDP variant
azlin gui install my-vm --protocol rdp

# Connect
azlin gui my-vm
```

`azlin gui` never installs implicitly. If nothing is installed it exits
non-zero and tells you the exact `azlin gui install` command to run.

### Protocols and clients

| `--protocol` | Container image | Server | Port (VM loopback) | Local client |
|---|---|---|---|---|
| `vnc` *(default)* | `consol/debian-xfce-vnc:v2.0.4` | TigerVNC (real RFB) | `127.0.0.1:5901` | any standard VNC viewer (`vncviewer`) |
| `rdp` | `lscr.io/linuxserver/rdesktop:ubuntu-xfce` | xrdp | `127.0.0.1:3389` | `xfreerdp`, Windows `mstsc`, macOS Remote Desktop |

Both images ship XFCE and have amd64 and arm64 variants. Recorded amd64 digests:

- `consol/debian-xfce-vnc:v2.0.4` — `sha256:b6d53e9f797bb4b4e3b7b317ec07e4242f33c7e3061af16d18685f6866295e58`
- `lscr.io/linuxserver/rdesktop:ubuntu-xfce` — `sha256:85f5e20fbed17a13be2619aafffedd6df2c3c68076693caf951176f133765062`

`linuxserver/webtop` was deliberately **not** used: it serves KasmVNC over
WebSockets, which a standard VNC viewer cannot speak.

For `rdp`, azlin looks for a local RDP client (`xfreerdp3`, `xfreerdp`,
`mstsc`) and launches it against the tunnel. If none is found it prints the
host, port, username and password so you can connect manually.

### The `install` subcommand vs. a VM named `install`

`azlin gui` accepts both a positional VM identifier and an `install`
subcommand, so a VM literally named `install` would be ambiguous. The
subcommand always wins; use the standard `--` separator to reach the VM:

```bash
azlin gui install      # the install subcommand
azlin gui -- install   # a VM named "install"
```

Note the flag is `--protocol vnc` / `--protocol=vnc` (clap syntax), not
`--protocol:vnc`.

### Install options

| Option | Default | Description |
|--------|---------|-------------|
| `--protocol` | `vnc` | `vnc` or `rdp` |
| `--resolution` | `1920x1080` | Desktop resolution (WIDTHxHEIGHT), VNC only |
| `--depth` | `24` | Colour depth (8, 16 or 24), VNC only |
| `--uninstall` | false | Remove the container and its state instead of installing |
| `--resource-group` | *(from session)* | Resource group |
| `--user` | `azureuser` | SSH username on the VM |
| `--key` | `~/.ssh/azlin_key` | Path to SSH private key |
| `-y, --yes` | false | Accepted for CLI compatibility; install is already non-interactive |

Install is **idempotent**. If a container already exists with the same image
and protocol it is simply started and the command reports
`already-installed`. Otherwise the old container is removed and recreated. The
container runs with `--restart unless-stopped`, so the desktop survives a VM
reboot.

### Connect options

| Option | Default | Description |
|--------|---------|-------------|
| `--resolution` | `1920x1080` | Accepted; the geometry is fixed at install time |
| `--depth` | `24` | Accepted; the depth is fixed at install time |
| `--user` | `azureuser` | SSH username on the VM |
| `--key` | `~/.ssh/azlin_key` | Path to SSH private key |
| `--minimal` | false | **Ignored** — the container owns its session. A warning is printed. |
| `--app` | none | **Ignored** — the container owns its session. A warning is printed. |
| `-y, --yes` | false | Compatibility flag |

`--minimal` and `--app` are accepted so existing scripts keep parsing, but a
containerised desktop manages its own session, so azlin warns rather than
silently dropping them.

### How it works

1. **Detect**: azlin runs a small read-only probe over the existing SSH or
   bastion connection. It reports whether Docker is present, whether the daemon
   is reachable as this user, and the state of the `azlin-gui` container. The
   probe always exits 0, so a non-zero exit unambiguously means an SSH
   transport failure, never "not installed".
2. **Install** (`azlin gui install` only): pulls the pinned image, generates a
   password **on the VM**, writes it to a `0600` env file under `~/.azlin/gui/`,
   and starts the container with the desktop port published on `127.0.0.1` only.
3. **Start**: if the container exists but is stopped, `azlin gui` starts it.
   That is a connect-time repair, not an install.
4. **Tunnel**: azlin forwards a local port to the container's loopback-bound
   port on the VM, through the same SSH or bastion tunnel it already uses.
5. **Launch**: the local VNC viewer or RDP client is pointed at
   `localhost:<local_port>`.
6. **Cleanup**: the tunnel is torn down when you disconnect. The container keeps
   running so the next connect is instant; remove it with
   `azlin gui install my-vm --uninstall`.

### Security

This is the strictest part of the design.

- **No NSG rule is ever created, modified or read.** There is no code path and
  no flag that opens 5901 or 3389 to the internet.
- **Loopback-only publish**: the container publishes
  `-p 127.0.0.1:5901:5901` (or `3389`). Even if a permissive NSG rule existed,
  the port is unreachable from off the VM.
- **noVNC (6901) is never published at all.**
- **Always authenticated**: a 32-hex-character password is generated on the VM
  with `openssl rand -hex 16` (falling back to `/dev/urandom`). The desktop is
  never left unauthenticated, even on localhost.
- **Password never in argv**: it is passed via `--env-file`, so it does not
  appear in `ps` or `docker inspect` output. The env file, the VNC auth blob and
  the RDP password file are all `0600` inside a `0700` `~/.azlin/gui/`.
- **Encrypted transport only**: all desktop traffic travels inside the SSH or
  bastion tunnel, which uses only port 22.

### Failure modes

The install script never swallows an error. Each failure exits with a distinct
code and azlin turns it into an actionable message:

| Exit | Meaning | Fix |
|---|---|---|
| 2 | Docker not installed on the VM | Recreate the VM with the azlin bootstrap, or install `moby-engine` |
| 3 | Docker daemon unreachable as this user | `sudo usermod -aG docker $USER`, then reconnect |
| 4 | Image pull failed | Check the VM's outbound network / registry reachability (the daemon's own error is included) |
| 5 | Password generation failed | Unexpected; check the VM's entropy sources |
| 6 | Container started but auth could not be configured | Re-run install; report if it repeats |
| 7 | Less than 4 GiB free on Docker's data root, and no larger disk to relocate to | Free disk, or attach a larger data disk |
| 8 | Desktop port already in use on the VM | Stop the conflicting listener |
| 9 | `docker run` failed | The daemon's own error text is included |

A zero exit **without** the expected completion marker is also treated as a
failure, so a silently-not-installed stack can never be reported as success.

### Troubleshooting

**`the remote desktop is not installed on this VM`**

Run the command azlin prints, e.g. `azlin gui install my-vm`.

**`Connection refused` when the viewer launches**

The tunnel may not be ready yet. Retry `azlin gui my-vm`.

**Screen resolution is wrong**

Geometry is baked in at install time. Re-run install with the resolution you
want:

```bash
azlin gui install my-vm --resolution 2560x1440
```

**Start over**

```bash
azlin gui install my-vm --uninstall
azlin gui install my-vm
```


## X11 Forwarding

Forward individual GUI windows from the VM to your local display. Best for lightweight apps where you don't need a full desktop.

### Usage

```bash
# Connect with X11 forwarding enabled
azlin connect --x11 my-vm

# Then on the VM, run any GUI app:
xeyes &
gitk --all &
meld file1 file2 &
```

### How It Works

1. `azlin connect --x11` adds the `-Y` flag (trusted X11 forwarding) to the SSH connection.
2. SSH sets up an encrypted tunnel for X11 protocol traffic.
3. The remote `DISPLAY` environment variable is set automatically by SSH.
4. GUI windows render on your local X server through the tunnel.
5. When connecting through Azure Bastion, the X11 tunnel is layered on top of the bastion tunnel seamlessly.

### Running Specific Applications

You can run any remote GUI app directly without opening an interactive session:

```bash
# Run a single app via X11 — app window appears locally
azlin connect my-vm --x11 --no-tmux -- chromium-browser --no-sandbox
azlin connect my-vm --x11 --no-tmux -- eog ~/screenshot.png
azlin connect my-vm --x11 --no-tmux -- thunar
azlin connect my-vm --x11 --no-tmux -- gitk --all
azlin connect my-vm --x11 --no-tmux -- meld file1.py file2.py
```

The `--no-tmux` flag avoids wrapping in tmux, and `--` separates azlin args from the remote command. The app renders locally and the connection closes when the app exits.

If you open an interactive X11 shell with `azlin connect --x11 my-vm` and then launch Chromium manually on an older VM, use `systemd-run --user --scope chromium-browser --no-sandbox` inside that shell. Newly provisioned azlin VMs install `/usr/local/bin/chromium-browser` and `/usr/local/bin/chromium` wrappers that add the required user-systemd scope automatically.

### Common GUI Applications

| Application | Command | Purpose |
|-------------|---------|---------|
| xeyes | `xeyes` | Quick test that X11 forwarding works |
| gitk | `gitk --all` | Visual git history browser |
| meld | `meld dir1 dir2` | Visual diff and merge tool |
| gedit | `gedit file.py` | Lightweight text editor |
| Chromium | `chromium-browser --no-sandbox` | Web browser (consider VNC for better performance) |
| eog | `eog image.png` | Image viewer |
| thunar | `thunar` | File manager |
| Firefox | `firefox` | Web browser (heavier, consider VNC) |
| VS Code | `code --disable-gpu` | Editor (use `--disable-gpu` over SSH) |

### X11 Troubleshooting

**`Error: Can't open display`**

The `DISPLAY` variable is not set on the VM. This usually means X11 forwarding was not enabled.

```bash
# Verify the connection was made with --x11
azlin connect --x11 my-vm

# On the VM, check DISPLAY is set
echo $DISPLAY
# Should show something like: localhost:10.0
```

**`X11 connection rejected because of wrong authentication`**

xauth cookies are mismatched. Regenerate them:

```bash
# On the VM
xauth generate $DISPLAY . trusted
```

**`Warning: No xauth data`**

The `xauth` package may be missing on the VM:

```bash
# On the VM
sudo dnf5 install -y xauth
# Disconnect and reconnect with --x11
```

**Apps are slow or laggy**

X11 forwarding sends individual draw commands over the network, which can be slow for complex UIs. Options:
- Use `ssh -C` (compression) if you need it on a manual SSH command.
- For heavy GUI usage, switch to VNC (`azlin gui`) which sends compressed screen updates instead.

## General Troubleshooting

### Bastion Tunnel Issues

Both X11 and VNC work through Azure Bastion tunnels. If connections fail when using Bastion:

```bash
# Verify Bastion tunnel is working
azlin bastion status my-bastion --resource-group my-rg

# Test basic SSH connectivity first
azlin connect my-vm

# Then try GUI forwarding
azlin connect --x11 my-vm
```

### Firewall / NSG Rules

No additional firewall or NSG rules are needed, and azlin never creates one. X11, VNC and RDP traffic all travel inside the SSH tunnel, which uses only port 22 (or the bastion tunnel). The desktop container publishes its port on the VM's loopback interface only.

### Performance Tips

- **Remote desktop**: Best for multi-app workflows or desktop environments. Choose a reasonable resolution at install time. Allow a few minutes for the first image pull (~1-2 GiB).
- **X11**: Best for lightweight apps (gitk, meld, xeyes). Avoid full browsers or IDEs.
- **Region proximity**: VMs in regions closer to you will have noticeably lower GUI latency.
- **VM size**: GUI rendering uses CPU; choose at least `Standard_D2s_v3` or above for a smooth experience.


## Disk layout on Azure Linux (measured on a live VM)

A default `azlin new` VM has a **4.7 GiB OS disk with ~1.1 GiB free**, while the
data disk mounted at `/mnt/home-data` has ~91 GiB free. The desktop images are
large (measured: VNC 2.82 GB, RDP ~2.7 GB, 5.52 GB with both pulled), so the
install would always fail on a stock VM.

`azlin gui install` therefore relocates container storage automatically when the
data root is too small **and Docker is pristine** (zero images, zero containers):

- Bind-mounts **both** `/var/lib/docker` *and* `/var/lib/containerd` onto the
  large disk. Relocating only Docker's `data-root` is **not** sufficient: modern
  moby extracts layers via containerd, whose root is not governed by
  `data-root`, so the pull still fails with `no space left on device`.
- Adds `/etc/fstab` entries so the relocation survives a reboot.
- Runs `restorecon -RF` on both paths. Azure Linux runs SELinux in **Enforcing**
  mode and a fresh bind mount arrives as `unlabeled_t`, which makes the
  container entrypoint fail with `exec ...: permission denied`.
- Never deletes existing data; it only mounts over an empty directory.

Measured on `Standard_D2s_v5` in northeurope: image pull ~19 s, full
`azlin gui install` ~95 s from a clean VM (including relocation), ~8 s for an
idempotent re-run, 42 s to switch protocols.

## Password entropy

The generated password is 32 hex characters. RDP uses all of it. The RFB (VNC)
protocol truncates passwords to **8 bytes**, so VNC authentication is ~32 bits
of entropy. This is acceptable only because port 5901 is bound to `127.0.0.1`
and is reachable exclusively through the SSH tunnel, so there is no network
brute-force surface.

## Verifying the security posture yourself

```sh
# on the VM: the desktop port must be loopback-bound
ss -ltnp | grep -E '5901|3389'
# -> LISTEN 0 4096 127.0.0.1:5901 0.0.0.0:*
```

When probing from outside, **use a banner grab, not `nc -z`**. Some networks
contain middleboxes that complete TCP handshakes indiscriminately, so `nc -z`
reports closed ports as open and produces a false "the desktop is exposed"
alarm. Only a genuine protocol banner (for example `SSH-2.0-...` on port 22)
proves a port is really reachable.
