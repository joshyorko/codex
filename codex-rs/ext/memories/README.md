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

## Setup and Management

Portable memory setup is built into Codex instead of routed through MCP or a
plugin. The runtime recall and writeback path stays direct, fail-open, and
available before/during/after turns; future plugins can add management surfaces
without becoming the core memory transport.

TUI flow:

- `/memory` opens the Memory settings panel.
- `/memory status` health-checks the selected backend/provider.
- `/memory setup codex-memoryd [--backend provider|hybrid] [--provider-url URL]`
  writes a local `codex-memoryd` provider config. When no URL is supplied, Codex
  uses `http://127.0.0.1:8787`.
- `/memory setup honcho [--backend provider|hybrid] [--honcho-api-key-env NAME]`
  writes a Honcho provider config. Codex stores the environment variable name,
  never the raw secret. When no name is supplied, Codex uses `HONCHO_API_KEY`.
- `/memory import-local preview` shows the local memory import payload without
  provider writes.
- `/memory import-local apply` uploads accepted local memory files only after
  explicit user action.
- `/memory disable` switches `memories.backend` back to `local` and leaves
  provider details in config for later reuse.

The settings panel exposes the same actions: status, setup `codex-memoryd`,
setup Honcho, switch to hybrid mode, preview/apply local import, disable
provider memory, and reset local memory.

CLI parity:

```sh
codex memory status
codex memory setup --provider codex-memoryd --backend provider --provider-url http://127.0.0.1:8787
codex memory setup --provider honcho --backend hybrid --honcho-api-key-env HONCHO_API_KEY
codex memory import-local --preview
codex memory import-local --apply
codex memory disable
```

Setup writes only the minimal `[memories]` keys needed by the selected flow:
`backend`, `provider`, `profile`, `workspace`, `user_peer`, `assistant_peer`,
`provider_url` when applicable, `honcho_api_key_env` for Honcho, and manual
import/visible-turn write policy defaults. Unrelated config is preserved through
the normal config editing machinery.

Health behavior:

- `codex-memoryd` setup/status checks the configured provider via the `/v1`
  adapter and the status path used by the provider.
- Honcho setup/status uses the existing provider read path and reports missing
  environment credentials or request failures as unreachable/unconfigured.
- Provider failures never block normal Codex startup. Provider-backed tools and
  runtime recall keep local fallback semantics where the selected backend allows
  it.

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
