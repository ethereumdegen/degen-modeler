# CONTRACT — slice interfaces (build-time coordination)

Plan of record: `~/ai/starkbot-neo/plans/18-degen-modeler.md`. This file pins
the cross-crate contracts for the parallel build. The spine (dgm-mesh core,
dgm-atlas pack, dgm-scene doc/op/ledger/dispatch/project) is written and
compiles; **read it before coding — types beat prose.**

## Invariants (every slice)

- Deterministic: `BTreeMap`/`BTreeSet` only where order can leak into output;
  no `HashMap` iteration into results; no randomness; no wall-clock anywhere
  except `ledger::append` provenance. Replaying `ops.jsonl` must reproduce a
  byte-identical `.glb`.
- All math `f32`, meters, +Y up, right-handed, CCW faces seen from outside.
- UV origin: top-left (image space), matching glTF. Per-corner UVs
  (`dgm_mesh::Corner.uv`).
- Errors: `thiserror`, message says which element ids ("v3", "f12", "e4:9").
- Gate findings: `dgm_mesh::Finding` with stable dotted `rule` ids
  (`mesh.*`, `uv.*`, `rig.*`, `anim.*`, `budget.*`), `Hard` blocks export.
- Ops mutate through `dgm_scene::dispatch::apply` only; ops never touch disk.
- NO stubs/`todo!()` in delivered code. If something can't be finished,
  report it — do not fake it.
- Do NOT run workspace-wide builds/tests mid-flight; build only your crate
  (`cargo check -p <crate>`), full workspace runs happen at integration.

## Slice map

| Slice | Owns | Must expose |
|---|---|---|
| MeshOps | `dgm-mesh/src/ops/{topo,deform}.rs` | see §MeshOps |
| UvAtlas | `dgm-uv/*`, `dgm-atlas/src/usage.rs` | see §UvAtlas |
| RenderMetrics | `dgm-render/*` | see §RenderMetrics |
| Gltf | `dgm-gltf/*` | see §Gltf |
| JevReview | `dgm-jev/*` | see §JevReview |
| ApiCli | `dgm-api/*`, `dgm-cli/*` | see §ApiCli |
| RigAnim | `dgm-scene/src/{rig_ops,anim_ops}.rs` (ONLY these files) | see §RigAnim |
| Ui | `dgm-ui/*` | see §Ui |

Integration (parent) wires dispatch arms currently returning
`OpError::Unrouted` to the slice functions, then builds the workspace.

## §MeshOps — `dgm_mesh::ops`

All: `&mut Mesh` + resolved element sets (scene resolves selections first),
return `Result<MeshDelta, MeshError>`; never leave the mesh in a state
`add_face` would refuse; call `prune_seams`/`remove_unused_verts` when faces
or verts vanish.

```rust
// ops/topo.rs
pub fn extrude(mesh: &mut Mesh, faces: &BTreeSet<FaceId>, offset: f32) -> Result<MeshDelta, MeshError>;
pub fn inset(mesh: &mut Mesh, faces: &BTreeSet<FaceId>, thickness: f32, depth: f32) -> Result<MeshDelta, MeshError>;
pub fn bevel_edges(mesh: &mut Mesh, edges: &BTreeSet<EdgeKey>, width: f32, segments: u32) -> Result<MeshDelta, MeshError>;
pub fn loop_cut(mesh: &mut Mesh, edge: EdgeKey, cuts: u32) -> Result<MeshDelta, MeshError>;
pub fn bridge(mesh: &mut Mesh, a: &BTreeSet<EdgeKey>, b: &BTreeSet<EdgeKey>) -> Result<MeshDelta, MeshError>;
pub fn merge_verts(mesh: &mut Mesh, verts: &BTreeSet<VertId>, distance: f32) -> Result<MeshDelta, MeshError>;
pub fn dissolve(mesh: &mut Mesh, faces: &BTreeSet<FaceId>) -> Result<MeshDelta, MeshError>;
pub fn mirror(mesh: &mut Mesh, axis: usize /*0=x,1=y,2=z*/, merge_distance: f32) -> Result<MeshDelta, MeshError>;
pub fn decimate_to_target(mesh: &mut Mesh, target_tris: u32) -> Result<MeshDelta, MeshError>; // QEM edge collapse
// ops/deform.rs
pub fn lattice(mesh: &mut Mesh, dims: [u32; 3], displacements: &[Vec3]) -> Result<MeshDelta, MeshError>;
```

Notes: extrude of a connected face patch moves the patch along the average
normal and stitches side quads on the patch boundary only. Mirror reflects
across the axis plane through the origin, flips winding on reflected faces,
merges verts within `merge_distance` of the plane. Decimate preserves
boundary/seam edges where possible and must produce a valid manifold-checked
mesh. Unit tests per op (topology counts + validate_mesh clean).

## §UvAtlas — `dgm-uv`

Operate on `&mut Mesh` (+`Pack` where regions matter). UV islands = faces
connected across non-seam edges (see `select::IslandOf` for the flood fill).

```rust
pub struct UvError(...);            // thiserror
pub fn project(mesh: &mut Mesh, faces: &BTreeSet<FaceId>, kind: &dgm_scene::ProjectKind) -> Result<MeshDelta, UvError>;
pub fn unwrap(mesh: &mut Mesh) -> Result<MeshDelta, UvError>;   // seams -> islands -> per-island planar fit + pack
pub fn assign_trim(mesh: &mut Mesh, faces: &BTreeSet<FaceId>, pack: &Pack, sheet: &str, region: &str) -> Result<MeshDelta, UvError>;
pub fn set_texel_density(mesh: &mut Mesh, faces: &BTreeSet<FaceId>, texels_per_meter: f32, texture_px: u32) -> Result<MeshDelta, UvError>;
pub fn pack_islands(mesh: &mut Mesh, margin_px: u32, texture_px: u32) -> Result<MeshDelta, UvError>;
pub fn islands(mesh: &Mesh) -> Vec<BTreeSet<FaceId>>;           // deterministic order
pub fn island_of(mesh: &Mesh, face: FaceId) -> BTreeSet<FaceId>;
// gate rules (called by dgm-jev::gate):
pub fn uv_findings(doc: &dgm_scene::Doc, pack: &Pack) -> Vec<Finding>;
//   uv.overlap        Hard: island bboxes overlap outside any declared MirrorSet
//   uv.out_of_bounds  Hard: UVs outside [0,1] for non-trim materials
//   uv.texel_band     Hard: face texel density outside pack band (needs material texture size)
//   uv.stretch        Warn: 3D/UV area ratio spread beyond 2x median
// dgm-atlas/src/usage.rs:
pub fn region_usage(doc: &Doc, pack: &Pack) -> BTreeMap<String /*sheet/region*/, Vec<String /*object*/>>;
```

`assign_trim` maps the selection's islands into the region's UV rect
(preserving island aspect, fitting the largest side). Texel density of a
face = sqrt(uv_area * (tex_px)^2 / world_area).

## §RenderMetrics — `dgm-render`

CPU rasterizer, `image::RgbaImage` out, deterministic pixel-for-pixel.
Perspective camera orbiting the scene bounds; unlit textured (nearest) with
a fixed dim-lambert term so shape reads; checkerboard background.

```rust
pub struct RenderError(...);
pub fn contact_sheet(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError>; // 4x2 = 8 yaw views, slight elevation
pub fn wireframe_sheet(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError>;
pub fn uv_layout(doc: &Doc, pack: &Pack, object: &str) -> Result<RgbaImage, RenderError>; // texture under island wires
pub fn heatmap(doc: &Doc, pack: &Pack, view_px: u32) -> Result<RgbaImage, RenderError>;   // texel density vs band (blue/green/red)
pub fn filmstrip(doc: &Doc, pack: &Pack, frames: u32, view_px: u32) -> Result<RgbaImage, RenderError>; // turntable strip
pub fn silhouette_masks(doc: &Doc, object: &str, views: u32, px: u32) -> Result<Vec<GrayImage>, RenderError>;
pub fn raycast(doc: &Doc, object: &str, origin: Vec3, dir: Vec3) -> Option<RayHit { face: FaceId, distance: f32, point: Vec3 }>;

#[derive(Serialize)] pub struct Metrics { /* your shape, stable field names */ }
pub fn metrics(doc: &Doc, pack: &Pack) -> Result<Metrics, RenderError>;
```

Metrics must include (names fixed): `seam_contrast_max`, `seam_contrast_mean`
(texture colour delta sampled across each UV seam edge, 0-1), `uv_occupancy`
(0-1 rasterized island coverage of [0,1]^2 for non-trim materials; null if
all-trim), `stretch_max`, `stretch_mean` (3D/UV texel ratio vs median),
`palette_distance` (mean nearest-palette RGB distance of used texels, 0-1),
`density_spread` (max/min face texel density), `mask_drift` (mean mask pixel
diff between LOD0 and highest LOD, 0-1; null without LODs), plus
`per_object` breakdowns. Golden PNG test for `contact_sheet` on a box.

## §Gltf — `dgm-gltf`

```rust
pub struct GltfError(...);
pub struct ExportOptions { pub embed_report: Option<serde_json::Value> } // -> glTF "extras"
pub fn export_glb(doc: &Doc, pack: &Pack, opts: &ExportOptions) -> Result<Vec<u8>, GltfError>;
pub fn import_summary(bytes: &[u8]) -> Result<serde_json::Value, GltfError>; // round-trip check: counts, materials, skins, anims
```

Deterministic bytes (fixed JSON key order, fixed buffer layout, no dates).
Per object: positions, per-corner normals (`corner_normals(pack.manifest.
hard_edge_angle_deg)`), UVs; corners with identical (pos,normal,uv) merge
into one glTF vertex. Materials: `KHR_materials_unlit` + lit fallback
(baseColorTexture/Factor both set), alphaMode MASK for `AlphaMode::Mask`,
doubleSided. Trim/File textures embed the PNG in the GLB buffer; Color
becomes baseColorFactor. Rigs -> skins (inverse bind from bone head/rest,
bones as node chain, weights VEC4/JOINTS_0). Clips -> animations (linear
sampler). LOD objects export as separate named nodes (`<name>_lod1`…) with
`MSFT_lod`-free plain naming (consumers pick by name). Tests: golden `.glb`
hash for a fixture doc + `gltf` crate re-import equals `import_summary`.

## §JevReview — `dgm-jev`

```rust
pub fn gate(doc: &Doc, pack: &Pack) -> GateReport; // mesh + budget + uv + rig + anim findings composed
#[derive(Clone, Copy, PartialEq)] pub enum Tier { Metrics, Jev, Vision }
pub struct ReviewOptions { pub tier: Tier }
/// dgm-jev does NOT depend on dgm-render: the caller (api/cli) renders and
/// measures, then hands everything in. Keeps slices decoupled + testable.
pub struct ReviewInputs<'a> {
  pub gate: GateReport,
  pub digest: serde_json::Value,          // dgm_scene::digest as JSON
  pub metrics: serde_json::Value,         // dgm_render::metrics as JSON
  pub goal: Option<&'a str>,
  pub sheet_png: Option<&'a [u8]>,        // vision tier only
  pub uv_png: Option<&'a [u8]>,
}
pub struct ReviewReport {   // Serialize
  pub gate: GateReport,
  pub heads: BTreeMap<String, f32>,       // silhouette, style, seams, waste, done
  pub tiers_ran: Vec<String>, pub skipped: Vec<SkippedTier { tier, reason }>,
  pub notes: Vec<String>,                  // vision prose, if it ran
  pub metrics: serde_json::Value,
  pub latency_ms: BTreeMap<String, u64>, pub cost_usd: Option<f64>,
}
pub async fn review(inputs: ReviewInputs<'_>, opts: &ReviewOptions) -> Result<ReviewReport, ReviewError>;
```

Tier chain (each optional tier degrades with a `skipped` entry, never an
error): 1) heads computed from `inputs.metrics` + gate by fixed formulas
(document them in code). 2) Jev: one POST
`{DGM_JEV_URL|https://api.typesafe.ai}/v1/systemone`, bearer
`TYPESAFE_API_KEY`, model `DGM_JEV_MODEL|"jev"`, body `{model, state,
questions}` where `state` is text: goal + digest JSON + metrics JSON;
`questions` = the five heads with instructions; answers 0-1 override the
formula heads. 5s timeout. 3) Vision (only when `opts.tier == Vision` and
an image is present): send goal + `sheet_png`/`uv_png` to
`OPENAI_API_KEY`@`OPENAI_BASE_URL|https://api.openai.com` (chat completions,
image_url data URLs, model `DGM_VISION_MODEL|"gpt-4o-mini"`) or
`ANTHROPIC_API_KEY` (messages API, image blocks, model
`DGM_VISION_MODEL|"claude-sonnet-4-5"`); parse strict JSON `{heads:{...},
notes:[...]}` out of the reply; merge. Keys via env only, into headers only.
Local CLIP scorer: post-v0, `skipped: "scorer: not built"` entry meanwhile
(README notes it). Artifacts named `r<rev>-<kind>.png`,
`r<rev>-review.json`. Wiremock tests for Jev + OpenAI paths.

## §ApiCli — `dgm-api` + `dgm-cli`

Axum, bind strictly `127.0.0.1:<port>` (default 7799), reject requests whose
`Host` isn't localhost. Shared state: `Arc<Mutex<Project>>`. JSON errors
`{error, detail?}` with proper status (400 bad op JSON, 404 unknown, 409
budget/gate, 502 upstream review).

```
GET  /health                    -> { ok, name, revision, class }
GET  /scene                     -> Digest (scene digest + `extra.metrics` merged when cheap)
GET  /pack                      -> pack manifest summary (budgets, trims+regions+sizes, rigs, clips, palette)
GET  /ledger?from=N             -> [LedgerLine]
GET  /object/{name}             -> ObjectDigest + islands + region usage
POST /op        {op json}       -> Outcome | error
POST /ops       [op json]       -> [Outcome] (stops at first error, reports index)
POST /review    {tier?}         -> ReviewReport (tier: "metrics"|"jev"|"vision", default jev)
POST /export    {out?}          -> { path, bytes, gate } ; 409 + report when gate fails
GET  /render/{kind}?object=&px= -> image/png (kind: sheet|wireframe|uv|heatmap|filmstrip) + X-Artifact header
GET  /artifacts/{file}          -> bytes
POST /raycast   {object, origin, dir} -> RayHit | null
```

`dgm` CLI (clap): `new <dir> --name --class --goal --pack <src>` (default
pack: `packs/classic` resolved from exe dir or `DGM_PACK`), `serve <dir>
--port`, `op <dir> <json|->`, `check <dir>` (gate; exit 1 on hard),
`review <dir> --tier`, `render <dir> --kind --out`, `export <dir> --out`,
`replay <dir>` (law: replay == live, compare export bytes; exit 1 on drift),
`digest <dir>`, `skill [--install <path>]` (emit SKILL.md describing the op
vocabulary + API for agent use). Actor strings: `api`, `cli`, `ui`.
Integration tests: spawn server on port 0, drive ops, assert digests.

## §RigAnim — `dgm-scene/src/{rig_ops,anim_ops}.rs` ONLY

```rust
// rig_ops.rs
pub fn rig_apply(doc: &mut Doc, pack: &Pack, object: &str, preset: &str, fit: &BTreeMap<String, BoneFit>) -> Result<Diff, OpError>;
pub fn rig_auto_weights(doc: &mut Doc, object: &str) -> Result<Diff, OpError>;
pub fn rig_paint_weights(doc: &mut Doc, sel: &Selection, bone: &str, value: f32, falloff: f32, mode: PaintMode) -> Result<Diff, OpError>;
pub fn rig_add_bone(doc: &mut Doc, object: &str, name: &str, parent: &str, head: [f32;3], tail: [f32;3]) -> Result<Diff, OpError>;
// anim_ops.rs
pub fn clip_apply(doc: &mut Doc, pack: &Pack, name: &str, object: &str, template: &str, speed: f32) -> Result<Diff, OpError>;
pub fn clip_key(doc: &mut Doc, clip: &str, bone: &str, time: f32, rot: Option<[f32;3]>, tr: Option<[f32;3]>, sc: Option<[f32;3]>) -> Result<Diff, OpError>;
pub fn clip_set_loop(doc: &mut Doc, clip: &str, looped: bool) -> Result<Diff, OpError>;
pub fn clip_delete(doc: &mut Doc, clip: &str) -> Result<Diff, OpError>;
```

`rig_apply`: scale/translate the preset's normalized bones (unit height,
feet y=0) into the mesh bounds; `fit` overrides individual bones in model
space. `rig_auto_weights`: nearest-bone-segment distance, up to 4
influences, inverse-distance falloff, normalized; deterministic.
`clip_apply`: instantiate the template (times * 1/speed, duration scaled;
euler degrees -> quaternions XYZ order; translations scaled by fitted rig
height). Must leave `rig_findings`/`clip_findings` clean on the happy path;
unit tests prove it on the pack biped.

## §Ui — `dgm-ui`

Bevy **0.19** + compatible bevy_egui. `dgm-ui <project-dir>` (also spawned
by `dgm ui`). Loads the Project directly (same crate APIs as the CLI; poll
`ops.jsonl` mtime each second and rebuild doc on change so an agent run
shows live). Viewport: doc meshes as Bevy meshes (unlit
StandardMaterial/unlit: true, textures from pack), orbit/pan/zoom, click
picking -> element id readout. Panels (bevy_egui): op ticker (last ledger
lines), digest summary (tris vs budget, gate state), review panel (heads +
findings of latest `artifacts/r*-review.json`), a JSON op console (text box
-> `project.apply`, actor "ui"). Engine-preview button: export via dgm-gltf
into `exports/preview.glb`, load through Bevy's glTF loader in a second
scene root. `--screenshot <path> --frames N` flag: render N frames headed,
save a window screenshot, exit — smoke proof. Skinned clip playback:
sample doc clips (linear) onto joint transforms.

## Ports & env

7799 API. `TYPESAFE_API_KEY`, `DGM_JEV_URL`, `DGM_JEV_MODEL`,
`OPENAI_API_KEY`, `OPENAI_BASE_URL`, `ANTHROPIC_API_KEY`,
`DGM_VISION_MODEL`, `DGM_PACK`.
