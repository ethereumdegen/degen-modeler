# Degen Modeler

Agent-first low-poly glTF modeling. Blender answers "what can a human
sculpt?"; Degen Modeler answers "what can an AI agent reliably build and
verify?" — a closed semantic op vocabulary, an append-only ledger as the
source of truth, a deterministic validation gate, and a cheap review loop
(Jev classifier + optional vision model) so an agent iterates
build → review → fix before a human ever looks.

Output: glTF 2.0 (`.glb`) in a classic-MMO style — low poly, hand-painted
trim sheets, deliberate UV economy. The style is a measurable contract in
`packs/classic/pack.json` (tri budgets, texel-density band, palette).

Plan of record: `~/ai/starkbot-neo/plans/18-degen-modeler.md`.

## Quick start

```sh
cargo build --release
target/release/dgm new /tmp/barrel --name barrel --class prop --goal "a wooden barrel"
target/release/dgm serve /tmp/barrel          # 127.0.0.1:7799
# drive it (any agent or curl):
curl -s localhost:7799/op -d '{"op":"prim_lathe","object":"barrel","segments":10,
  "profile":[[0.0,0.0],[0.30,0.0],[0.38,0.35],[0.30,0.72],[0.0,0.72]]}'
curl -s localhost:7799/scene | jq .budget
curl -s localhost:7799/review -d '{"tier":"metrics"}' | jq .heads
curl -s -X POST localhost:7799/export -d '{}'
```

Human UI (Bevy 0.19): `dgm-ui /tmp/barrel` — viewport, op ticker, review
panel, engine preview of the exported `.glb`, clip playback. Humans and
agents write to the same ledger; edits interleave.

## Layout

| Crate | Role |
|---|---|
| `dgm-mesh` | indexed low-poly kernel: stable ids, topology ops, validators |
| `dgm-atlas` | style packs: budgets, palette, trim sheets, rig presets, clip templates |
| `dgm-scene` | doc, op vocabulary, ledger, dispatch, rigs/clips, project on disk |
| `dgm-uv` | projections, unwrap, trim binding, texel density, packing, UV gate |
| `dgm-render` | deterministic CPU rasterizer: contact sheets, UV/heatmap views, metrics |
| `dgm-gltf` | deterministic `.glb` export (unlit + lit fallback, skins, animations) |
| `dgm-jev` | review stack: gate composition, Jev (TypeSafe) heads, vision reviewer |
| `dgm-api` | axum on `127.0.0.1:7799` — the primary product surface |
| `dgm-cli` | `dgm` new/serve/op/check/review/render/export/replay/digest/skill |
| `dgm-ui` | Bevy viewport + bevy_egui panels over the same op layer |

## Laws (enforced, tested)

- Ledger revisions strictly increase; replaying `ops.jsonl` reproduces a
  byte-identical export (`dgm replay` proves it).
- Export only from a doc that passes the deterministic gate; the report
  rides in glTF `extras`.
- Budget-breaching ops fail closed, naming the overage.
- API binds `127.0.0.1` only.
- Review models are advisory: no op or export blocks on a classifier or
  vision answer or outage. Vision runs only at escalation points and is
  priced per call.

## Review tiers

1. **Gate** (free, deterministic, blocks export): manifold, budgets, UV
   overlap outside declared mirror sets, texel band, rig/anim validity.
2. **Metrics** (free, deterministic): seam contrast, UV occupancy, stretch,
   palette distance, silhouette drift.
3. **Jev** (`TYPESAFE_API_KEY`, ~150 ms): fuses digest + metrics into five
   heads — silhouette, style, seams, waste, done.
4. **Vision** (`OPENAI_API_KEY`/`ANTHROPIC_API_KEY`, on demand): judges the
   actual renders, returns the same heads plus prose notes.

Local CLIP scorer for tier 2 is planned (plan §6) and not yet built; the
report lists it as skipped.

## Env

`TYPESAFE_API_KEY`, `DGM_JEV_URL`, `DGM_JEV_MODEL`, `OPENAI_API_KEY`,
`OPENAI_BASE_URL`, `ANTHROPIC_API_KEY`, `DGM_VISION_MODEL`, `DGM_PACK`.
