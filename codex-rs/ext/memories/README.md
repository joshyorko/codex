# Codex Memories Extension

This crate owns Codex memory prompt context, memory tools, and portable memory
lifecycle hooks.

Local memory remains the default and preserves upstream behavior. Portable
memory is in-process Codex runtime code, not a sidecar or wrapper.

## Architecture

- `runtime.rs` owns per-thread portable memory lifecycle, per-turn recall, and
  visible-turn writeback buffering.
- `provider.rs` defines the durable provider contract used by the runtime.
- `selected.rs` selects local, provider-backed, or hybrid tool behavior.
- `policy.rs` contains portable memory safety filters and metadata.
- `portable_schema.rs` contains portable memory domain types.
- `schema.rs` remains the tool schema helper module.
- `honcho.rs` implements the first non-local provider.
- `local.rs` remains the upstream local filesystem backend.

The intended shape is:

```text
PortableMemoryRuntime
  -> MemoryProvider
    -> Honcho provider
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
backend = "honcho"
profile = "personal"
workspace = "codex-memory-lab"
user_peer = "josh"
assistant_peer = "codex"
honcho_api_key_env = "HONCHO_API_KEY"
```

For local development against a local Honcho-compatible service:

```toml
[memories]
backend = "honcho"
workspace = "codex-memory-lab"
honcho_base_url = "http://localhost:8000/v3"
```

Hybrid mode keeps local memory files useful as cache/debug surface while syncing
durable operations to the configured provider when available:

```toml
[memories]
backend = "hybrid"
profile = "oss"
workspace = "codex-memory-lab"
honcho_api_key_env = "HONCHO_API_KEY"
```

Portable memory fields:

- `backend = "local" | "honcho" | "hybrid"`
- `profile = "personal" | "work" | "oss" | "homelab"`
- `write_policy = "off" | "visible_turns"`
- `sync_policy = "manual" | "startup"`
- `cross_profile_policy = "default_deny"`

## Development

On Josh's Bluefin workstation, prefer the repo devcontainer or project
container over host Rust setup. From the Codex repo root:

```sh
devcontainer up --workspace-folder .
```

Inside the container, run focused gates:

```sh
just test -p codex-config memories_config_defaults_to_local_backend memories_config_parses_honcho_backend_fields
just test -p codex-memories-extension
just write-config-schema
just fmt
```

Because this crate adds Rust dependencies, also run from the repo root:

```sh
just bazel-lock-update
just bazel-lock-check
```
