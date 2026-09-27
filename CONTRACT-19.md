# CONTRACT-19 — organic environments slices (plan 19)

Plan: `~/ai/starkbot-neo/plans/19-organic-environments.md`. Spine already
landed (read it, types beat prose): `Mesh.colors: BTreeMap<VertId,[f32;3]>`
(linear RGB, absent = white), `Material.emissive/emissive_strength`,
`Doc.references: Vec<String>`, `Doc.tags: BTreeMap<object, BTreeSet<tag>>`
(`contact`, `open`), `AssetClass::Environment` (budget 12000 in the classic
pack), and every new `Op` variant in `dgm-scene/src/op.rs` (dispatch arms
currently `Unrouted`; integration wires them to your functions).

Invariants from CONTRACT.md apply (determinism, BTree order, no stubs, test
only your crates, never workspace-wide builds).

## OrganicMesh — `dgm-mesh/src/ops/organic.rs` + `primitives.rs` additions + `Mesh::append`

```rust
pub fn subdivide(mesh: &mut Mesh, levels: u32, smooth: bool) -> Result<MeshDelta, MeshError>;
//   Catmull-Clark (any polygon); smooth=false = linear midpoint split. Seams: split onto sub-edges. UVs: interpolated. Colors: interpolated.
pub fn displace_noise(mesh: &mut Mesh, verts: &BTreeSet<VertId>, amplitude: f32, scale: f32, seed: u64) -> Result<MeshDelta, MeshError>;
//   3-octave value/fbm noise (hash-based, deterministic) sampled at vert pos/scale, moves vert along its area-weighted normal by amplitude*noise(-1..1).
pub fn smooth(mesh: &mut Mesh, verts: &BTreeSet<VertId>, iterations: u32, factor: f32) -> Result<MeshDelta, MeshError>;
//   Laplacian toward neighbour centroid; boundary verts pinned; factor 0..1.
pub fn solidify(mesh: &mut Mesh, thickness: f32) -> Result<MeshDelta, MeshError>;
//   Offset copy along -vertex-normal (thickness>0 = inward), flipped winding, rim quads along boundary edges; copies UVs/seams/colors; result manifold-checked (validate_mesh clean).
pub fn snap_to_surface(mesh: &mut Mesh, verts: &BTreeSet<VertId>, target: &Mesh, dir: Vec3) -> Result<MeshDelta, MeshError>;
//   For each vert: nearest hit of the ray (both ±dir) against target's triangles; move vert there. Verts with no hit are reported in the error message ids and left in place (Err only if NONE hit).
impl Mesh { pub fn append(&mut self, other: &Mesh) -> BTreeMap<VertId, VertId> } // remaps ids, carries faces/uvs/seams/colors; returns old->new vert map
// primitives.rs
pub fn tunnel(path: &[Vec3], radii: &[f32], segments: u32) -> Result<Mesh, MeshError>;
//   Parallel-transport frames along the polyline (>=2 points), one ring per point, quads between rings, OPEN ends. Default UVs: u = arc around, v = distance along; seam column like lathe. radii.len() == 1 or == path.len().
pub fn cavern(radii: Vec3, segments: u32, rings: u32, floor_y: Option<f32>, noise: f32, seed: u64) -> Result<Mesh, MeshError>;
//   UV-sphere ellipsoid centered at origin, faces entirely below floor_y removed (open floor), then displace_noise(all, noise*min radius, 0.5*min radius, seed). Outward normals. Default UVs = spherical unroll (u = theta*r_eq, v = phi*r) + apex fans seamed like lathe; spread islands.
```
Tests: subdivide face/vert counts + validate clean; solidify of an open box lid → closed manifold with 2x faces + rim; snap drops a floating vert onto a plane; tunnel/cavern validate clean, counts exact, uvs non-degenerate.

## Bake — `dgm-scene/src/bake_ops.rs` + `dgm-gltf` COLOR_0/emissive + `dgm-render` vertex color + `dgm-ui` viewport colors

```rust
// dgm-scene/src/bake_ops.rs (scene-level: occlusion against ALL objects)
pub fn bake_ao(doc: &mut Doc, object: &str, samples: u32, strength: f32) -> Result<Diff, OpError>;
//   Per-vertex hemisphere sampling (deterministic sample set, e.g. Fibonacci), ray-vs-all-scene triangles with a simple BVH/grid (must stay < 2 s for 12k tris x 5k verts x 32 samples); color = lerp(white, ao, strength) MULTIPLIED into existing colors.
pub fn bake_sun(doc: &mut Doc, object: &str, dir: [f32;3], strength: f32, color: [f32;3]) -> Result<Diff, OpError>;
//   Half-lambert on vertex normal vs -dir, tinted by color, multiplied in; no shadows (AO covers occlusion).
pub fn bake_glow(doc: &mut Doc, object: &str, lights: &[String], radius: f32, strength: f32) -> Result<Diff, OpError>;
//   For each light object with an emissive material: additive tint by emissive*strength*falloff(dist to its bounds center, radius).
pub fn paint_vertex(doc: &mut Doc, sel: &Selection, color: [f32;3], strength: f32, facing: Option<[f32;3]>, max_angle_deg: f32) -> Result<Diff, OpError>;
//   Blend toward color by strength on the selection's verts (faces → verts whose adjacent face normal passes the facing test).
```
- dgm-gltf: emit `COLOR_0` (u8 normalized VEC3; only when the mesh has any colors; missing verts = white); materials: `emissiveFactor` + `KHR_materials_emissive_strength` when strength != 1; `import_summary` gains `vertex_colors: bool`, `emissive_materials: n`. Re-pin golden hash (document the reason).
- dgm-render: multiply interpolated vertex color into the unlit sample in every view (beauty sheet, filmstrip); heatmap/uv/wireframe unaffected; `metrics.lit_range` = (min,max,mean luminance of vertex colors across the scene, null if none).
- dgm-ui viewport.rs + preview: set `Mesh::ATTRIBUTE_COLOR` from `Mesh.colors` (Bevy multiplies automatically for StandardMaterial); dgm-view needs nothing (glTF COLOR_0 loads natively).
Tests: AO on a vertex inside a box corner darker than one on an open plane; sun on ±Y faces ordered; paint_vertex facing filter; gltf reimport reports colors + emissive.

## SceneGate — `dgm-jev/src/scene_rules.rs` (+ hook into `gate()`)

```rust
pub fn scene_findings(doc: &Doc, pack: &Pack) -> Vec<Finding>;
// scene.intersects   Hard for Building/Environment, Warn otherwise: tri-tri intersection between two objects unless either carries tag `contact` (or is a LOD). Bbox-cull pairs first; report the pair + up to 8 face ids each.
// scene.floating     Warn: object whose min distance to every other object's triangles > 0.02 m (skip if only one object; skip LODs).
// mesh.open_boundary Hard for Building/Environment when an object has boundary edges and lacks tag `open`; Warn otherwise. (Cutout planes = tag open.)
// mesh.inverted      Hard: closed object with negative signed volume (winding inside-out).
```
Provide a `cave_fixture()` in tests reproducing today's failure modes (floor plane through a dome rim, floating cone, open shell without tag) and assert each rule fires; and a clean joined manifold passes. Wire `scene_findings` into `gate()` after uv rules.

## EnvReview — `dgm-render` interior views + env metrics, `dgm-jev` critique with references + env prompt, `dgm-api`/`dgm-cli` plumbing, SKILL docs for every new op

- dgm-render: `pub fn interior_sheet(doc, pack, view_px) -> Result<RgbaImage, RenderError>`: 4 views from inside the scene bounds (from the +Z edge looking in, from the center looking +Z/-Z, looking up), near-plane clamped; `metrics.connectivity` (fraction of objects that touch another within 0.02 m), `metrics.repetition` (normalized autocorrelation peak of the beauty sheet's texture detail, 0..1).
- dgm-jev critique: `CritiqueInputs` gains `references: Vec<(String, Vec<u8>)>` (label, png/jpg bytes) and `environment: bool`; env prompt asks continuity/scale/lighting/focal points; references listed as "reference image N (style target)"; heads gain `reference_match` when references are present.
- dgm-api: `/render/interior`, critique orchestration loads `doc.references` (project-relative; skip unreadable with a note), adds the interior sheet for Building/Environment; `/scene` digest shows references + tags.
- dgm-cli `skill.rs`: OpDoc entries for ALL 16 new ops (set_reference, tag_object, prim_tunnel, prim_cavern, subdivide, displace_noise, smooth, solidify, join, snap_to_surface, bake_ao, bake_sun, bake_glow, paint_vertex, clear_vertex_colors — and material_new's new fields), plus an "Environments playbook" section (cavern/tunnel → solidify → join → snap formations → bake ao+sun+glow → interior render → critique with reference). The existing `skill_covers_every_op_exactly` test must pass.

## Integration (parent)
Dispatch arms, `Unrouted` removal, workspace build, cave v2 rebuild as acceptance.
