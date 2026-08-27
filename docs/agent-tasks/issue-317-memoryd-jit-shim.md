# Agent execution prompt — issue #317

You are implementing **joshyorko/codex#317** on this branch.

Primary issue:
- https://github.com/joshyorko/codex/issues/317

Parent product/protocol issue:
- https://github.com/joshyorko/codex-memoryd/issues/228

Related UX follow-up:
- https://github.com/joshyorko/codex/issues/63

This file is a **steering artifact for this disposable PR branch**. Read it completely before editing code. Before opening the final PR, remove this file so the final patch contains only product code/tests/JIT-maintenance artifacts that belong in the Codex patch.

---

## Mission

Produce the **smallest mechanically renewable Codex shim** that gives `codex-memoryd` automatic cross-session continuity on current upstream Codex:

1. bounded, fail-open MemoryD recall before a turn;
2. safe visible-turn/host-context contribution after a turn;
3. native Codex memory remains intact and can coexist;
4. MemoryD is disabled by default and stock Codex behavior is unchanged when disabled;
5. the shim can be recreated on a fresh upstream checkout and fails loudly when an owned upstream seam changes.

This is **not** a request to resurrect `tap-release`.

The fork is an intermediary patch/test point. The long-lived product is `codex-memoryd`; the Codex integration must stay disposable.

---

## Branch/base rule — do this first

This branch was created from upstream Codex SHA:

```text
694edc23b22b4696400dc47663ecacd437623870
```

That SHA was current `openai/codex@main` when the task was prepared on 2026-08-27.

**Before any implementation work:**

1. fetch `openai/codex` current `main`;
2. record its SHA in your work log / PR body;
3. if upstream has advanced, rebase/reset this disposable branch onto the latest upstream `main` **before** product edits;
4. do not inherit `tap-release` or other fork-only commits into the base;
5. verify the fork base is a clean upstream lineage.

If you cannot establish a clean current-upstream base, stop implementation and report the exact Git condition rather than building on stale history.

---

## Read current upstream before old code

Inspect these current-upstream areas first:

```text
codex-rs/ext/extension-api/src/contributors.rs
codex-rs/ext/extension-api/src/registry.rs
codex-rs/ext/memories/src/extension.rs
codex-rs/ext/memories/src/backend.rs
codex-rs/ext/memories/src/local.rs
codex-rs/ext/memories/src/prompts.rs
codex-rs/app-server/src/extensions.rs
codex-rs/config/src/types.rs
codex-rs/core/src/config/
```

Also inspect the current native memory generation/consolidation path enough to understand whether MemoryD-injected context can accidentally become native-memory source evidence.

Current upstream exposes official extension seams including:

- `ContextContributor`
- `TurnInputContributor`
- `TurnLifecycleContributor`
- `TurnItemContributor`
- `ThreadLifecycleContributor`
- `ToolContributor`
- `ConfigContributor`

Prefer those seams over core-runtime edits.

Only **after** understanding current upstream, inspect historical behavior:

```text
branch: tap-release
commit: 02df45df629c8a6683ac9f163bafbf105d2d2016
  "Add portable memory provider runtime"

branch: patchraptor/codex-memoryd-provider-conformance
commit: 1b117a6b5a1ced2f1aba4f5b5cbd8528b8ef24a4
  "Add codex-memoryd provider conformance tests"
```

Mine those for **requirements, failure behavior, and test cases**. Do not mechanically port the old architecture.

---

## Architecture decision you must make before coding

Compare these two current-main designs:

### A. Dedicated MemoryD extension

Likely shape:

```text
codex-rs/ext/memoryd/
  config
  bounded HTTP client
  TurnInputContributor for recall
  TurnItemContributor / TurnLifecycleContributor for visible writeback
  tests
```

with minimal workspace/config/extension-registry wiring.

### B. Extend current memories extension with a remote/selected backend

Reuse/extend `MemoriesBackend` or a minimal selected-backend enum if this produces a smaller, more upstream-native seam.

Do **not** assume B just because the June branch used a provider abstraction. Current upstream `MemoriesBackend` is primarily a memory-tool storage interface; verify whether stretching it to own automatic turn recall/writeback makes architectural sense.

### Selection rule

Choose the design with:

1. fewer core-owned files/symbols;
2. better use of official extension hooks;
3. cleaner disabled=stock behavior;
4. easier mechanical regeneration on upstream changes;
5. less coupling to native Codex memory internals.

Put a short architecture decision/rationale in the PR body.

**Stop and redesign** if the chosen implementation starts requiring broad prompt/runtime/event-loop modifications outside extension/config registration seams.

---

## Behavioral contract

### Pre-turn recall

When MemoryD is enabled/configured:

- issue a bounded request to MemoryD for relevant context before model execution;
- derive query/input only from current model-visible user input + safe explicit host metadata;
- carry configured profile/workspace and repo identity when safely available;
- use a strict timeout;
- on timeout/unreachable/error/malformed response, proceed without MemoryD;
- inject returned material as contextual recall, never instruction authority;
- preserve a recognizable `recall_not_authority` boundary in model-visible rendering;
- never send hidden reasoning/private scratch state to MemoryD.

When MemoryD is disabled/unconfigured:

- no MemoryD network request;
- stock Codex memory behavior remains unchanged.

### Post-turn contribution

After/during a completed turn, contribute only safe **visible** content through the MemoryD protocol.

Preferred eventual MemoryD primitive from `codex-memoryd#228`:

```text
memory_observe / host continuity contribution
```

If that protocol is not yet landed when you implement this PR:

- isolate the MemoryD client behind a narrow internal trait/type;
- temporarily map visible turns to the current `/v1/turns` endpoint if necessary;
- make swapping to `memory_observe` a client-local change, not another Codex-core rewrite.

Rules:

- visible user content is eligible;
- visible assistant content is eligible;
- hidden chain-of-thought/reasoning is never eligible;
- arbitrary raw tool output is not automatically eligible;
- secrets/credential-like content must not be intentionally persisted;
- writeback failure is fail-open and never rolls back/fails the user's turn.

### Strong writes stay separate

Do not collapse these semantics:

```text
host observation      -> weak continuity evidence
explicit conclusion   -> durable user/workflow conclusion
checkpoint            -> resumable task/work state
```

The first PR may expose only the automatic observation/writeback path, but internal types must leave room for the stronger operations rather than pretending every observation is a conclusion.

---

## Fresh black-box memory stress-test findings — preserve these semantics

A fresh-thread ChatGPT memory/search stress test run on 2026-08-27 produced an important negative result:

```text
2023 actual prior-chat hits recovered: none
2024 actual prior-chat hits recovered: none
2025 actual prior-chat hits recovered: none
```

The system still recovered useful **other historical context** for 2023 and 2024, used that context to materially change current web-search ranking, and then made longitudinal inferences. For 2025 it found insufficient technical evidence and **abstained** instead of projecting known 2026 preferences backward.

This means useful memory behavior is not one undifferentiated bucket. At minimum the product architecture must be able to distinguish:

```text
raw/recent episode or prior-chat evidence
host-native recalled/synthesized context
MemoryD durable memory / observation
other historical evidence
model inference / longitudinal synthesis
current verified state
```

The exact MemoryD vocabulary is owned by `codex-memoryd#228`; do not invent a competing Codex ontology. The Codex shim's responsibility is to **preserve provenance and temporal distinctions provided by MemoryD rather than flattening everything into anonymous prompt text**.

### Required Codex-side implications

1. If MemoryD recall returns source kind/class, evidence refs, memory/episode ids, confidence, observed time, valid-time/freshness state, or lineage metadata, do not discard it unnecessarily at the adapter boundary.
2. Rendering may remain compact, but the model-visible fragment should make important epistemic distinctions legible where practical: e.g. `recent episode`, `durable memory`, `historical/contextual`, `inferred`, and always `recall_not_authority`.
3. Keep returned MemoryD ids/lineage in turn-local extension state when needed so a later host contribution can say it was **derived from prior MemoryD recall** rather than masquerading as new independent evidence.
4. If the assistant visibly repeats or reasons from a recalled MemoryD claim, post-turn contribution must not accidentally make that a fresh independent confirmation. Prefer forwarding `derived_from_memory_ids`/lineage when #228 exposes it.
5. Do not assign a historical timestamp merely because a current model statement describes the past. Preserve explicit source/observed time when available; otherwise mark time unknown/current-observation rather than fabricating chronology.
6. Newer context must not be silently projected backward into older periods. This policy primarily belongs in MemoryD, but the Codex adapter must preserve timestamps/source classes so MemoryD can enforce it.
7. Missing provenance makes evidence weaker, not stronger.
8. Do not label host-native recollection as a verbatim transcript hit unless the source actually provides a raw/episode reference supporting that claim.

### Add focused canaries where the seam can test them

The Codex PR is not responsible for implementing MemoryD's full temporal reasoning engine, but it should prove the adapter does not destroy the information required for that engine. Add tests for:

- provenance/source-class fields from a MemoryD response survive parsing/rendering or turn-local state as designed;
- returned MemoryD ids can be carried into derived writeback lineage when supported;
- recalled MemoryD content is not automatically submitted back as an independent primary observation simply because the assistant used it;
- unknown/missing provenance does not get upgraded to a stronger source class;
- temporal metadata supplied by MemoryD is preserved rather than rewritten to “now.”

If #228 has not landed the required lineage fields yet, keep these as typed optional fields / explicit TODO contract tests or file a precise blocker back to #228. Do not broaden Codex core to compensate for a missing MemoryD protocol field.

---

## Native Codex memory coexistence matrix

Add deterministic coverage for these modes:

```text
native memory OFF + MemoryD OFF
native memory ON  + MemoryD OFF
native memory OFF + MemoryD ON
native memory ON  + MemoryD ON
```

Required properties:

- OFF/OFF is stock behavior;
- ON/OFF remains stock native-memory behavior;
- OFF/ON proves MemoryD works independently;
- ON/ON proves no obvious double-injection or lifecycle breakage.

Investigate whether current upstream native memory generation reads only original thread/user-visible history or can consume extension-injected recall as source material. If there is any route for MemoryD recall to be re-learned as fresh native evidence, either prevent it at the seam or add explicit provenance/boundary handling and a regression test.

Do not disable native Codex memory in this task.

---

## TDD / RED-first requirement

Before implementation, create failing focused tests for the chosen seam. Port test **intent** from historical provider-conformance work where useful.

Minimum canaries:

- disabled MemoryD performs zero provider calls;
- enabled MemoryD performs pre-turn recall;
- recall appears in the intended model-visible slot;
- timeout fails open;
- connection failure fails open;
- non-2xx / malformed response fails open with bounded diagnostics;
- user-visible content can be contributed;
- assistant-visible content can be contributed;
- hidden reasoning cannot be contributed;
- writeback failure does not fail the turn;
- profile/workspace stay stable;
- native-memory coexistence matrix passes;
- MemoryD context is visibly/non-semantically marked recall-not-authority;
- response/error body handling is bounded;
- source/provenance metadata is not silently flattened away;
- MemoryD-derived output cannot become false independent evidence through a trivial read/write round trip.

Use a scripted loopback HTTP fixture. Unit/conformance tests must not depend on a live external MemoryD instance.

---

## Keep the patch tiny

Prefer almost all new behavior to live in one owned extension/module.

Changes outside the owned MemoryD module should ideally be limited to:

- Cargo/workspace registration;
- extension registry install line;
- minimal typed config;
- narrowly necessary exports.

If you are editing many unrelated core files, stop and reconsider the design.

Do not add a generic provider marketplace/framework unless current upstream makes that strictly smaller than the MemoryD-specific shim.

No TUI wizard in this PR. `joshyorko/codex#63` owns later setup UX.

---

## JIT/mechanical-refresh deliverable

The PR must include a small, deterministic seam-maintenance artifact under an appropriate owned path, e.g.:

```text
contrib/codex-memoryd/
  README.md
  seam-manifest.json
  check.sh
```

Names/location may change to match repo conventions, but the contract may not disappear.

`seam-manifest.json` or equivalent must record at least:

```text
upstream_base_sha
shim/version identifier
owned files
owned upstream symbols/interfaces relied upon
MemoryD endpoint/capability assumptions
focused verification commands
```

The check must be usable by a future automation that does roughly:

```text
fetch latest upstream
apply/reconstruct shim
run focused seam tests
classify result
```

Classification must distinguish at least:

```text
compatible
upstream_seam_drift
memoryd_protocol_mismatch
ordinary_test_failure
```

Use a stable marker such as:

```text
MEMORYD_CODEX_SEAM_DRIFT
```

when an expected owned upstream interface/symbol has disappeared or changed incompatibly.

Do not build an autonomous merge-conflict resolver. The point is **mechanical detection + bounded repair**, not pretending all upstream changes can be safely auto-fixed.

---

## Patch transport choice

Before finalizing, decide the simplest renewable representation:

- one/few cherry-pickable shim commits;
- generated `.patch` series;
- deterministic reconstruction script;
- another equivalently small representation.

Measure it against the goal: a fresh upstream checkout should be able to recreate the integration without carrying a perpetually rebased fork branch.

Document the chosen refresh command in `contrib/codex-memoryd/README.md` (or equivalent).

---

## Verification

At minimum run the exact focused test packages affected by the final design.

Likely commands may include variants of:

```bash
cd codex-rs
cargo fmt --check
cargo test -p codex-memories-extension
cargo test -p codex-extension-api
```

If you add a dedicated crate, run its complete tests explicitly.

Also run the smallest relevant app-server/core integration test set needed to prove extension registration and model-visible contribution behavior.

Do not claim success from compilation alone.

Record:

- upstream base SHA;
- focused tests run;
- broader tests run/not run;
- known limitations;
- exact patch/shim refresh command;
- diff/touched-file count.

---

## PR shape

Open **one focused PR** against `joshyorko/codex` for issue #317.

PR title should clearly identify this as the disposable/JIT MemoryD shim, not a generic Codex memory rewrite.

PR body must include:

- `Closes #317`;
- link to `joshyorko/codex-memoryd#228`;
- current upstream base SHA;
- chosen architecture and why;
- touched upstream seams;
- conformance results;
- mechanical refresh procedure;
- native-memory coexistence result;
- provenance/lineage preservation result from the fresh memory-stress-test requirements;
- explicit statement that `tap-release` code was used only as historical behavioral evidence.

Before opening the PR:

1. remove this steering file (`docs/agent-tasks/issue-317-memoryd-jit-shim.md`);
2. verify the PR diff does not contain stale fork history;
3. verify the patch is based on current upstream lineage;
4. verify disabled MemoryD = stock behavior;
5. run the focused seam check one final time.

Do not merge the PR yourself unless separately instructed.

---

## Non-goals

Do not:

- rebase/restore `tap-release` as a maintained branch;
- rewrite native Codex memory/Dreaming;
- implement ChatGPT App functionality here;
- implement MemoryD server protocol changes here unless a blocker is first identified and filed in `codex-memoryd`;
- route the native Codex integration through MCP when direct extension hooks exist;
- add hidden prompt injection;
- capture/store hidden reasoning;
- add the provider TUI/setup wizard;
- open an upstream OpenAI PR unless explicitly instructed later;
- optimize unrelated Codex code.

---

## Completion standard

You are done when a clean current-upstream Codex tree can acquire the MemoryD integration as a small deterministic shim, pass focused behavioral canaries, and return to stock behavior simply by disabling/removing the shim. Future upstream changes must either pass the compatibility check or fail with a precise owned-seam drift signal so an agent can repair the bounded seam without “boiling the ocean.”
