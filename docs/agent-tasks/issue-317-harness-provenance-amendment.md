# Issue #317 steering amendment — first-class harness provenance

This amendment is **binding** for `joshyorko/codex#317` and complements `docs/agent-tasks/issue-317-memoryd-jit-shim.md` plus the canonical protocol issue `joshyorko/codex-memoryd#228`.

## Core distinction

MemoryD must identify the **application / agent harness that produced or contributed evidence**. Do not collapse producer identity into model identity, UI surface, MemoryD adapter, transport, or a generic free-form `source` string.

An LLM/API call by itself is not the provenance boundary. The trusted application layer is.

Required conceptual dimensions:

```text
producer_class      agent_harness | chat_application | importer | manual | other
harness_id          codex | hermes | claude_code | chatgpt | ...
surface_id          codex_cli | t3code | chatgpt_web | chatgpt_desktop | ...
adapter_id          memoryd_native_extension | memoryd_mcp | memoryd_chatgpt_app | importer | ...
transport           native_http | mcp | stdio | import | ...
harness_version     optional
model_provider      optional telemetry only
model_id            optional telemetry only
session_id          optional
thread_id           optional
source_kind         visible_turn | host_native_recall | host_checkpoint | conclusion | memoryd_derived | ...
```

Exact field names may follow repository conventions. The semantic separation may not disappear.

## Codex requirements

For automatic contributions made by this extension, trusted Codex code should stamp approximately:

```text
producer_class = agent_harness
harness_id = codex
adapter_id = memoryd_native_extension
transport = native_http
```

Add `surface_id` only when the surface can be established reliably. Never infer it from model text.

### T3 Code example

When T3 Code is the UI driving the Codex harness, preserve both layers:

```text
harness_id = codex
surface_id = t3code
adapter_id = memoryd_native_extension
```

Do not flatten that to `source=t3code`, because the execution/memory-aware harness was Codex.

If Hermes later uses the same model, its evidence must remain distinguishable by `harness_id=hermes`. A shared model/provider does not make the producer the same.

## Incoming recall provenance

Do not relabel recalled MemoryD evidence as Codex-origin merely because Codex consumed it.

Example:

```text
original claim provenance: harness_id = hermes
current consumer:           harness_id = codex
```

The original provenance stays Hermes. If Codex later emits a visible statement derived from that recall, the new contribution should carry:

```text
producer harness = codex
derived_from_memory_ids = [...]
lineage = existing lineage
```

so MemoryD can distinguish **origin of the claim** from **the harness currently repeating/using it**.

## Invariants

1. Harness/application identity is primary producer provenance; model identity is optional secondary telemetry.
2. Missing/unknown harness provenance becomes weaker/unknown; never guess `codex`, `chatgpt`, etc. from model/provider/cwd/repo/user-agent text.
3. The trusted application/adapter layer asserts its own producer identity; the model does not choose it.
4. Producer/harness provenance must survive evidence, episode/observation projection, Dreamer promotion, durable memory, recall/export, and round trips where semantically relevant.
5. Dedupe/lineage may record independent observations from distinct harnesses, but a MemoryD-derived claim must not gain confidence merely by round-tripping through Codex, Hermes, ChatGPT, or another host.
6. Imported ChatGPT history must remain distinguishable from live ChatGPT App contributions.
7. Model/provider fields must not be used as a substitute for harness provenance or as authority by default.

## Required focused tests for #317

- outgoing automatic contribution stamps `harness_id=codex` from trusted extension code;
- model/provider fields can vary without changing Codex harness identity;
- incoming non-Codex provenance survives recall parsing/state and is not rewritten to Codex;
- derived writeback can preserve returned MemoryD ids/lineage alongside the new Codex producer identity;
- unknown surface/harness-adjacent metadata is left unknown rather than guessed;
- the temporary `/v1/turns` adapter keeps this provenance typed internally so migration to #228 `memory_observe` / host-contribution fields is localized.

Do not block the first shim PR on final #228 wire-field names. Create a narrow typed internal provenance object and isolate the wire mapping.