# VM OS Image Selection

Choose the operating system image for new VMs via the `--os` flag or persistent configuration.

## Overview

By default, `azlin new` provisions VMs with Azure Linux 4.0. You can override this per-command with `--os` or set a persistent default with `azlin config set default_vm_image`.

## Quick Start

```bash
# Use Azure Linux 4.0 for this VM
azlin new --name my-vm --os azurelinux4

# Set a persistent default
azlin config set default_vm_image "azurelinux4"

# Now all new VMs use Azure Linux 4.0
azlin new --name my-vm
```

## The `--os` Flag

```bash
azlin new --os <IMAGE_SPEC> [OTHER_OPTIONS]
```

`IMAGE_SPEC` accepts two formats:

### Shorthands

Convenient aliases for the Azure Linux 4.0 image:

| Shorthand | Resolved Image URN |
|-----------|-------------------|
| `azurelinux4` | `MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest` |
| `azure-linux-4` | `MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest` |
| `azurelinux` | `MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest` |
| `4.0` | `MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest` |

All supported shorthands resolve to the Azure Linux 4.0 Gen2 image.

### Full Image URN

Azure image URNs in the format `Publisher:Offer:SKU:Version`:

```bash
azlin new --os "MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest"
```

Only images from the `MicrosoftCBLMariner` publisher are accepted. Non-MicrosoftCBLMariner URNs are rejected because azlin's provisioning defaults target Azure Linux 4.0 and `dnf5`.

## Configuration

### Setting a Default Image

```bash
# Set default using a shorthand
azlin config set default_vm_image "azurelinux4"

# Set default using a full URN
azlin config set default_vm_image "MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest"

# View current default
azlin config get default_vm_image

# Remove default (revert to built-in Azure Linux 4.0)
azlin config unset default_vm_image
```

The value is validated on `set` — invalid shorthands or malformed URNs are rejected. The resolved full URN is stored in `~/.azlin/config.toml`.

### Config File

```toml
# ~/.azlin/config.toml

# Default OS image for new VMs (full URN or shorthand)
default_vm_image = "MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest"

# Other defaults
default_region = "westus2"
default_vm_size = "Standard_E16as_v5"
default_resource_group = "azlin-vms"
```

## Priority Chain

When creating a VM, the OS image is resolved in this order (highest priority first):

1. **`--os` flag** — per-command override
2. **`default_vm_image` config** — persistent default in `~/.azlin/config.toml`
3. **Built-in default** — Azure Linux 4.0 (`MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest`)

```bash
# Uses --os flag (highest priority)
azlin new --os azurelinux4

# Uses config default_vm_image (if set)
azlin new

# Uses built-in Azure Linux 4.0 (if no config set and no --os)
azlin new
```

## Examples

### Create a VM with Azure Linux 4.0

```bash
azlin new --name dev-vm --os azurelinux4
```

### Create a pool with a specific image

```bash
azlin new --pool 3 --name build-fleet --os azurelinux4
```

### Set team-wide default via config

```bash
# All team members run this once
azlin config set default_vm_image "azurelinux4"

# Then just use azlin new normally
azlin new --name my-vm
```

### Override config default for one VM

```bash
# Config says Azure Linux 4.0, but you need an explicit override for this VM
azlin new --name test-vm --os azurelinux4
```

### Use full URN for a specific image version

```bash
azlin new --name pinned-vm --os "MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest"
```

## Input Validation

Image specifications are validated for safety:

- **Shorthands** must match a supported Azure Linux 4.0 alias
- **Full URNs** must have exactly 4 colon-separated segments
- **Publisher** must be `MicrosoftCBLMariner` (non-Azure Linux 4.0 images are rejected)
- **Segments** may only contain `[a-zA-Z0-9._-]` characters
- **Shell metacharacters**, newlines, and null bytes are rejected

Invalid input produces a clear error:

```
$ azlin new --os "NotAPublisher:image:sku:latest"
Error: Only MicrosoftCBLMariner publisher is supported for VM images, got "NotAPublisher".
  Use a URN like 'MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest'

$ azlin new --os "not-a-version"
Error: Unknown image shorthand "not-a-version". Supported shorthands:
  azurelinux4, azure-linux-4, azurelinux, 4.0.
  Or use a full URN like 'MicrosoftCBLMariner:azure-linux-4:azure-linux-4-gen2:latest'
```

## Troubleshooting

### "Unknown OS image shorthand"

You used a shorthand that isn't recognized. Check the [shorthands table](#shorthands) or use a full URN.

### "Only MicrosoftCBLMariner Azure Linux 4.0 images are supported"

azlin requires Azure Linux 4.0 because its provisioning defaults use `dnf5`. Use a MicrosoftCBLMariner image URN or a recognized shorthand.

### Config default not taking effect

Check that `default_vm_image` is set correctly:

```bash
azlin config show
```

Verify the `--os` flag isn't overriding it (it has higher priority).

## See Also

- [Quick Reference](../QUICK_REFERENCE.md) — All CLI flags at a glance
- [Configuration Reference](../reference/config-default-behaviors.md) — All config options
- [Region Fit](region-fit.md) — Auto-find regions with available quota
