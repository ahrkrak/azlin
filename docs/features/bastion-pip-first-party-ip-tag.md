# First-Party Usage IP Tag for Bastion Public IPs (Opt-In)

azlin can tag the Azure public IP it creates for a bastion host with an Azure
**IP tag**, such as `FirstPartyUsage=/ATEVETNonProd`. This satisfies the
first-party usage tagging requirement for non-production Azure resources.

> **This is opt-in and disabled by default.** Unless you set
> `bastion_pip_ip_tags` in `~/.azlin/config.toml`, azlin passes no `--ip-tags`
> argument at all. See [Why It Is Opt-In](#why-it-is-opt-in).

## What is the First-Party Usage IP Tag?

Azure public IP addresses support **IP tags** — typed key/value metadata that
the platform recognizes for billing, compliance, and routing classification. IP
tags are distinct from ordinary Azure resource tags: they are applied at IP
allocation time via the `--ip-tags` argument of
`az network public-ip create` and use the form `<TagType>=<TagValue>`.

The first-party tag used inside Microsoft is:

| Field     | Value             |
| --------- | ----------------- |
| Tag type  | `FirstPartyUsage` |
| Tag value | `/ATEVETNonProd`  |

Which corresponds to the argument:

```
--ip-tags FirstPartyUsage=/ATEVETNonProd
```

## Why It Is Opt-In

`FirstPartyUsage` is a Microsoft-internal first-party IP tag. Supplying **any**
first-party IP tag requires the target subscription to be registered for the
feature `Microsoft.Network/AllowBringYourOwnPublicIpAddress`.

Microsoft-internal subscriptions have that feature registered. Ordinary
subscriptions (Pay-As-You-Go, Visual Studio Enterprise, MSDN, CSP, most
enterprise agreements) do **not** — for them the feature is `NotRegistered`, and
the Azure control plane rejects the request outright:

```
ERROR: (SubscriptionNotRegisteredForFeature) Subscription
/subscriptions/<id>/resourceGroups//providers/Microsoft.Network/subscriptions/
is not registered for feature
Microsoft.Network/AllowBringYourOwnPublicIpAddress required to carry out the
requested operation.
```

Earlier versions of azlin applied this tag unconditionally. That made bastion
public IP creation — and therefore `azlin bastion` and the bastion pre-check
auto-create flow — fail 100% of the time on any subscription without the
feature. The tag is now opt-in so azlin works everywhere by default, while
Microsoft-internal users can still get the tag by setting one config key.

Note: the malformed-looking resource ID in the error above (`resourceGroups//`
with an empty segment) comes from Azure's own error message. It does not mean
azlin passed an empty resource group.

## Enabling the Tag

Set `bastion_pip_ip_tags` in `~/.azlin/config.toml`:

```toml
# ~/.azlin/config.toml
bastion_pip_ip_tags = "FirstPartyUsage=/ATEVETNonProd"
```

or via the CLI:

```bash
azlin config set bastion_pip_ip_tags "FirstPartyUsage=/ATEVETNonProd"
```

or per-invocation with the environment variable:

```bash
AZLIN_BASTION_PIP_IP_TAGS="FirstPartyUsage=/ATEVETNonProd" azlin new --name my-vm
```

The value is a free-form `Key=Value` IP tag passed through verbatim as the value
of `--ip-tags`, so any valid Azure IP tag works, not just `FirstPartyUsage`:

```toml
bastion_pip_ip_tags = "RoutingPreference=Internet"
```

### Resolution order

1. `AZLIN_BASTION_PIP_IP_TAGS` environment variable, when set to a valid tag.
   Setting it to the **empty string explicitly disables** the tag, overriding the
   config file.
2. The persisted `bastion_pip_ip_tags` config field, when non-empty.
3. The default: empty — **no `--ip-tags` argument at all**.

An environment value that fails validation is ignored with a warning and
resolution falls through to the config field, then the default.

### Validation

When a non-empty value is supplied it must be a well-formed `Key=Value` IP tag:
the key must be non-empty, must not begin with `-` (an `az` CLI flag-injection
guard), the whole value must be at most 512 characters, and must contain no
control characters. An empty or whitespace-only value is valid and means
"disabled".

Only enable the tag if
`az feature show --namespace Microsoft.Network --name AllowBringYourOwnPublicIpAddress`
reports `Registered` for your subscription.

## Relationship to upstream `rysweet/azlin`

Upstream made this value configurable in PR #1039 (`865753a7`, merged
2026-07-13), and this implementation deliberately mirrors its naming — the
`bastion_pip_ip_tags` config field, the `AZLIN_BASTION_PIP_IP_TAGS` environment
variable, the `AzlinConfig::bastion_pip_ip_tags()` resolver, the
`validate_bastion_pip_ip_tags()` validator, and the
`build_create_pip_args(rg, region, ip_tags)` signature — so the change stays a
clean upstream contribution and minimises future merge conflicts.

It diverges from upstream on two points, deliberately:

| Behaviour | Upstream | Here |
| --------- | -------- | ---- |
| Default value | `FirstPartyUsage=/ATEVETNonProd` | empty (no tag) |
| Empty value | rejected as invalid; resolver falls back to the default and `--ip-tags` is **always** emitted | valid; means "disabled", and `--ip-tags` is omitted entirely |

Upstream's resolver is documented as always returning a non-empty, valid tag, so
upstream still cannot turn the tag off. That means upstream still fails 100% of
the time on any subscription that is not registered for
`Microsoft.Network/AllowBringYourOwnPublicIpAddress` — it made the value
configurable without making the feature usable outside Microsoft-internal
tenants. Defaulting to "no tag" is the only setting that works everywhere.

## Usage

The tag, when configured, is applied whenever azlin provisions bastion
infrastructure — for example, during the bastion pre-check auto-create flow or
any command that ensures bastion infrastructure exists:

```bash
azlin new --name my-vm --region eastus2 --size xl
# If bastion infrastructure is missing, azlin creates it. The bastion public IP
# azlin-bastion-eastus2-pip is allocated with the IP tag from
# bastion_pip_ip_tags, or with no IP tag at all when that key is unset.
```

Under the hood, azlin runs the equivalent of the following. **The `--ip-tags`
line is present only when `bastion_pip_ip_tags` is set**; by default the whole
line is omitted:

```bash
az network public-ip create \
  --resource-group <rg> \
  --name azlin-bastion-eastus2-pip \
  --location eastus2 \
  --sku Standard \
  --allocation-method Static \
  --ip-tags FirstPartyUsage=/ATEVETNonProd \
  --output none
```

> The `--output none` flag suppresses the command's output, so running the
> command above prints nothing on success. To inspect the resulting IP tag, use
> the `show` query in [Verifying the Tag](#verifying-the-tag).

## Verifying the Tag

You can confirm the IP tag on any bastion public IP with the Azure CLI:

```bash
az network public-ip show \
  --resource-group <rg> \
  --name azlin-bastion-eastus2-pip \
  --query ipTags \
  --output json
```

Expected output when `bastion_pip_ip_tags` is set:

```json
[
  {
    "ipTagType": "FirstPartyUsage",
    "tag": "/ATEVETNonProd"
  }
]
```

With the default configuration the query returns `[]`.

## Scope and Behavior

- **Default**: No IP tag. No `--ip-tags` argument is passed to the Azure CLI.
- **Applies to**: All bastion public IPs created by azlin once
  `bastion_pip_ip_tags` (or `AZLIN_BASTION_PIP_IP_TAGS`) is set.
- **Idempotent**: The tag value comes from config; repeated bastion provisioning
  with unchanged config always produces the same tag.
- **Non-destructive**: The tag is additive. All other public IP settings (SKU,
  allocation method, location, naming) are unchanged.
- **Does not retroactively modify** public IPs that were created earlier.
  Pre-existing IPs can be tagged manually if needed.

## Configuration Reference

| Setting                      | Type     | Default        | Meaning                              |
| ---------------------------- | -------- | -------------- | ------------------------------------ |
| `bastion_pip_ip_tags`        | `string` | `""` (no tag)  | Value passed to `--ip-tags`          |
| `AZLIN_BASTION_PIP_IP_TAGS`  | env var  | unset          | Overrides the config field; empty disables |

The value is a non-sensitive compliance/billing identifier and is safe to appear
in command output and logs. It is passed as a discrete `argv` element (not a
shell string), so there is no command-injection surface.

## Related

- [Bastion Pre-Check for Private VMs](bastion-pre-check.md)
- [How to set up bastion infrastructure](../how-to/setup-bastion-infrastructure.md)
- [Configuration Reference](../reference/configuration-reference.md)
