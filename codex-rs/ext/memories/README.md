# Codex Memories Extension

This crate owns Codex memory prompt context, memory tools, and portable memory
lifecycle hooks.

Local memory remains the default and preserves upstream behavior. The Codex
integration is in-process runtime code, not an external wrapper; durable memory
can be supplied by local, HTTP, or future provider implementations.

## Architecture

- `runtime.rs` owns per-thread portable memory lifecycle, per-turn recall, and
  visible-turn writeback buffering.
- `provider.rs` defines the durable provider contract used by the runtime.
- `selected.rs` selects local, provider-backed, or hybrid tool behavior.
- `policy.rs` contains portable memory safety filters and metadata.
- `portable_schema.rs` contains portable memory domain types.
- `schema.rs` remains the tool schema helper module.
- `honcho.rs` implements the first non-local provider.
- `codex_memoryd.rs` implements the Codex-native HTTP provider adapter.
- `local.rs` remains the upstream local filesystem backend.

The intended shape is:

```text
PortableMemoryRuntime
  -> MemoryProvider
    -> Honcho provider
    -> codex-memoryd provider
    -> future native or hosted providers
```

## Configuration

Local mode is the default:

```toml
[memories]
backend = "local"
```

Honcho mode uses Honcho as the durable provider and fails open when required
configuration is missing or unavailable:

```toml
[memories]
backend = "provider"
provider = "honcho"
profile = "personal"
workspace = "codex-memory-lab"
user_peer = "josh"
assistant_peer = "codex"
honcho_api_key_env = "HONCHO_API_KEY"
```

For local development against a local Honcho-compatible service:

```toml
[memories]
backend = "provider"
provider = "honcho"
workspace = "codex-memory-lab"
provider_url = "http://localhost:8000/v3"
```

`backend = "honcho"` and `honcho_base_url` remain compatibility aliases for
existing Honcho configs.

codex-memoryd mode calls the documented `/v1` HTTP provider endpoints:

```toml
[memories]
backend = "provider"
provider = "codex_memoryd"
provider_url = "http://127.0.0.1:8787"
local_import_policy = "manual"
```

Hybrid mode keeps local memory files useful as cache/debug surface while syncing
durable operations to the configured provider when available:

```toml
[memories]
backend = "hybrid"
provider = "honcho"
profile = "oss"
workspace = "codex-memory-lab"
honcho_api_key_env = "HONCHO_API_KEY"
```

Portable memory fields:

- `backend = "local" | "provider" | "hybrid"`
- `provider = "honcho" | "codex_memoryd"`
- `provider_url = "http://127.0.0.1:8787"` for HTTP providers
- `profile = "personal" | "work" | "oss" | "homelab"`
- `write_policy = "off" | "visible_turns"`
- `local_import_policy = "prompt" | "manual" | "startup_preview" | "startup_apply"`
- `sync_policy = "manual" | "startup"` remains a compatibility setting
- `cross_profile_policy = "default_deny"`

## Development

On Josh's Bluefin workstation, prefer the repo devcontainer or project
container over host Rust setup. From the Codex repo root:

```sh
devcontainer up --workspace-folder .
```

Inside the container, run focused gates:

```sh
just test -p codex-config memories_config
just test -p codex-memories-extension
just write-config-schema
just fmt
```

Because this crate adds Rust dependencies, also run from the repo root:

```sh
just bazel-lock-update
just bazel-lock-check
```
