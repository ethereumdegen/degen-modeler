//! SKILL.md: the document an agent reads to drive Degen Modeler. Generated
//! from `OP_FAMILIES` so the op list and the shipped doc cannot drift apart;
//! tests assert the table matches `dgm_scene::op::Op` exactly and that every
//! example parses.

pub struct OpDoc {
    pub name: &'static str,
    pub what: &'static str,
    pub example: &'static str,
}

pub struct OpFamily {
    pub title: &'static str,
    pub ops: &'static [OpDoc],
}

pub const OP_FAMILIES: &[OpFamily] = &[
    OpFamily {
        title: "Primitives",
        ops: &[
            OpDoc {
                name: "prim_box",
                what: "Axis-aligned box centered at the origin; size is [x,y,z] meters.",
                example: r#"{"op":"prim_box","object":"crate","size":[1.0,1.0,1.0]}"#,
            },
            OpDoc {
                name: "prim_plane",
                what: "Single quad in the XZ plane facing +Y; size is [x,z] meters.",
                example: r#"{"op":"prim_plane","object":"ground","size":[2.0,2.0]}"#,
            },
            OpDoc {
                name: "prim_cylinder",
                what: "Cylinder around +Y; `caps` (default true) closes the ends.",
                example: r#"{"op":"prim_cylinder","object":"barrel","radius":0.4,"height":1.0,"segments":12}"#,
            },
            OpDoc {
                name: "prim_lathe",
                what: "Revolve a [radius, y] profile around +Y (vases, poles, domes).",
                example: r#"{"op":"prim_lathe","object":"vase","profile":[[0.0,0.0],[0.3,0.2],[0.2,0.8],[0.0,1.0]],"segments":10}"#,
            },
            OpDoc {
                name: "prim_ngon_prism",
                what: "N-sided prism around +Y (hex pillars, bolt heads).",
                example: r#"{"op":"prim_ngon_prism","object":"pillar","sides":6,"radius":0.5,"height":2.0}"#,
            },
        ],
    },
    OpFamily {
        title: "Topology",
        ops: &[
            OpDoc {
                name: "extrude",
                what: "Move the selected face patch along its average normal, stitching side quads on the patch boundary.",
                example: r#"{"op":"extrude","sel":{"q":"faces_facing","object":"crate","dir":[0.0,1.0,0.0]},"offset":0.25}"#,
            },
            OpDoc {
                name: "inset",
                what: "Shrink each selected face inward by `thickness`, optionally pushing in by `depth` (panel lines).",
                example: r#"{"op":"inset","sel":{"q":"faces","object":"crate","ids":[5]},"thickness":0.05,"depth":0.02}"#,
            },
            OpDoc {
                name: "bevel",
                what: "Chamfer the selected edges; `segments` rounds the profile.",
                example: r#"{"op":"bevel","sel":{"q":"edges","object":"crate","pairs":[[0,1]]},"width":0.03,"segments":1}"#,
            },
            OpDoc {
                name: "loop_cut",
                what: "Insert `cuts` parallel edge loops through the quad ring containing edge [a,b].",
                example: r#"{"op":"loop_cut","object":"crate","edge":[0,1],"cuts":2}"#,
            },
            OpDoc {
                name: "bridge",
                what: "Connect two edge loops with a quad tube (named or inline selections).",
                example: r#"{"op":"bridge","sel_a":"loop_top","sel_b":"loop_bottom"}"#,
            },
            OpDoc {
                name: "merge_verts",
                what: "Weld selected vertices closer than `distance` meters.",
                example: r#"{"op":"merge_verts","sel":{"q":"all","object":"crate","kind":"verts"},"distance":0.001}"#,
            },
            OpDoc {
                name: "dissolve",
                what: "Remove the selected faces' shared edges, merging them into larger faces.",
                example: r#"{"op":"dissolve","sel":{"q":"faces","object":"crate","ids":[3,4]}}"#,
            },
            OpDoc {
                name: "mirror",
                what: "Reflect the object across the axis plane through the origin, welding within `merge_distance`.",
                example: r#"{"op":"mirror","object":"crate","axis":"x"}"#,
            },
            OpDoc {
                name: "flip_normals",
                what: "Reverse winding on the selected faces (fix inside-out shells).",
                example: r#"{"op":"flip_normals","sel":{"q":"faces","object":"crate","ids":[2]}}"#,
            },
            OpDoc {
                name: "decimate_to_budget",
                what: "Edge-collapse the object down to `target_tris` triangles.",
                example: r#"{"op":"decimate_to_budget","object":"crate","target_tris":300}"#,
            },
        ],
    },
    OpFamily {
        title: "Deform",
        ops: &[
            OpDoc {
                name: "translate",
                what: "Move the selection by `delta` meters; `proportional` spreads falloff over that radius.",
                example: r#"{"op":"translate","sel":{"q":"all","object":"crate"},"delta":[0.0,0.5,0.0]}"#,
            },
            OpDoc {
                name: "rotate",
                what: "Rotate the selection around `axis` by `degrees`, about `origin` (default: selection centroid).",
                example: r#"{"op":"rotate","sel":{"q":"all","object":"crate"},"axis":"y","degrees":45.0}"#,
            },
            OpDoc {
                name: "scale",
                what: "Scale the selection by per-axis `factors` about `origin` (default: selection centroid).",
                example: r#"{"op":"scale","sel":{"q":"all","object":"crate"},"factors":[1.0,2.0,1.0]}"#,
            },
            OpDoc {
                name: "lattice",
                what: "Deform via a dims[0]*dims[1]*dims[2] control grid over the bounds; one displacement per control point (x fastest).",
                example: r#"{"op":"lattice","object":"crate","dims":[2,2,2],"displacements":[[0.0,0.0,0.0],[0.0,0.0,0.0],[0.0,0.0,0.0],[0.0,0.0,0.0],[0.1,0.0,0.0],[0.1,0.0,0.0],[0.1,0.0,0.0],[0.1,0.0,0.0]]}"#,
            },
            OpDoc {
                name: "snap_to_grid",
                what: "Round selected vertex positions to a `step`-meter grid.",
                example: r#"{"op":"snap_to_grid","sel":{"q":"all","object":"crate","kind":"verts"},"step":0.05}"#,
            },
        ],
    },
    OpFamily {
        title: "Selection",
        ops: &[OpDoc {
            name: "select_save",
            what: "Save a query under a name; later ops can pass the name as `sel`.",
            example: r#"{"op":"select_save","name":"top_faces","query":{"q":"faces_facing","object":"crate","dir":[0.0,1.0,0.0],"max_angle_deg":30.0}}"#,
        }],
    },
    OpFamily {
        title: "UV",
        ops: &[
            OpDoc {
                name: "mark_seams",
                what: "Mark the selected edges as UV seams (island boundaries for unwrap).",
                example: r#"{"op":"mark_seams","sel":{"q":"edges","object":"crate","pairs":[[0,1],[1,2]]}}"#,
            },
            OpDoc {
                name: "clear_seams",
                what: "Unmark seams on the selected edges.",
                example: r#"{"op":"clear_seams","sel":{"q":"edges","object":"crate","pairs":[[0,1]]}}"#,
            },
            OpDoc {
                name: "uv_unwrap",
                what: "Seams -> islands -> per-island planar fit + pack into [0,1]^2.",
                example: r#"{"op":"uv_unwrap","object":"crate"}"#,
            },
            OpDoc {
                name: "uv_project",
                what: "Project UVs onto the selection: kind planar|box|cylindrical; `axis` is required for planar/cylindrical and forbidden for box.",
                example: r#"{"op":"uv_project","sel":{"q":"all","object":"crate"},"kind":"box"}"#,
            },
            OpDoc {
                name: "uv_assign_trim",
                what: "Map the selection's UV islands into a named region of a pack trim sheet.",
                example: r#"{"op":"uv_assign_trim","sel":{"q":"faces_facing","object":"crate","dir":[0.0,1.0,0.0]},"sheet":"wood","region":"planks"}"#,
            },
            OpDoc {
                name: "uv_assign_rect",
                what: "Map the selection's UV islands into an arbitrary [u0,v0,u1,v1] rect — region layout on an owned (File-texture) atlas.",
                example: r#"{"op":"uv_assign_rect","sel":{"q":"all","object":"trunk","kind":"faces"},"rect":[0.02,0.02,0.48,0.98],"texels_per_meter":275.0}"#,
            },
            OpDoc {
                name: "uv_declare_mirror",
                what: "Declare intentionally overlapping islands (mirrored parts) so `uv.overlap` allows them.",
                example: r#"{"op":"uv_declare_mirror","name":"arms","sel":{"q":"faces","object":"hero","ids":[10,11]}}"#,
            },
            OpDoc {
                name: "uv_set_texel_density",
                what: "Rescale the selection's islands to a texel density (texels per meter).",
                example: r#"{"op":"uv_set_texel_density","sel":{"q":"all","object":"crate"},"texels_per_meter":256.0}"#,
            },
            OpDoc {
                name: "uv_pack",
                what: "Repack the object's islands into [0,1]^2 with a pixel margin.",
                example: r#"{"op":"uv_pack","object":"crate","margin_px":4}"#,
            },
        ],
    },
    OpFamily {
        title: "Material",
        ops: &[
            OpDoc {
                name: "material_new",
                what: "Create a material: texture is {kind:\"trim\",sheet} | {kind:\"file\",path} | {kind:\"color\",rgba}; alpha opaque|mask. `emissive` [r,g,b] (linear) + `emissive_strength` (default 1) make crystals/lamps glow (exported as emissiveFactor + KHR_materials_emissive_strength; `bake_glow` reads them).",
                example: r#"{"op":"material_new","name":"crystal","texture":{"kind":"color","rgba":[110,200,255,255]},"emissive":[0.3,0.7,1.0],"emissive_strength":3.0}"#,
            },
            OpDoc {
                name: "object_material",
                what: "Bind an existing material to an object.",
                example: r#"{"op":"object_material","object":"crate","material":"wood"}"#,
            },
        ],
    },
    OpFamily {
        title: "Rig",
        ops: &[
            OpDoc {
                name: "rig_apply",
                what: "Fit a pack rig preset (biped, quadruped, prop_hinge, chain) into the mesh bounds; `fit` overrides bones in model space.",
                example: r#"{"op":"rig_apply","object":"hero","preset":"biped"}"#,
            },
            OpDoc {
                name: "rig_auto_weights",
                what: "Nearest-bone-segment weights, up to 4 influences, normalized.",
                example: r#"{"op":"rig_auto_weights","object":"hero"}"#,
            },
            OpDoc {
                name: "rig_paint_weights",
                what: "Set/add/smooth a bone's weight on the selected verts with a falloff radius.",
                example: r#"{"op":"rig_paint_weights","sel":{"q":"in_box","object":"hero","min":[-1.0,0.0,-1.0],"max":[1.0,0.5,1.0],"kind":"verts"},"bone":"spine","value":0.8,"falloff":0.1,"mode":"replace"}"#,
            },
            OpDoc {
                name: "rig_add_bone",
                what: "Extend a rig with one bone (tails, banners); `parent` must exist.",
                example: r#"{"op":"rig_add_bone","object":"hero","name":"tail1","parent":"hips","head":[0.0,0.9,-0.1],"tail":[0.0,0.9,-0.4]}"#,
            },
        ],
    },
    OpFamily {
        title: "Animation",
        ops: &[
            OpDoc {
                name: "clip_apply",
                what: "Instantiate a pack clip template (idle, walk, open, sway) onto a rigged object; `speed` rescales time.",
                example: r#"{"op":"clip_apply","name":"walk","object":"hero","template":"walk","speed":1.0}"#,
            },
            OpDoc {
                name: "clip_key",
                what: "Set one key on a clip channel: euler degrees rotation, translation, and/or scale at `time`.",
                example: r#"{"op":"clip_key","clip":"walk","bone":"arm_l","time":0.5,"rotation_euler_deg":[0.0,0.0,45.0]}"#,
            },
            OpDoc {
                name: "clip_set_loop",
                what: "Toggle looping on a clip.",
                example: r#"{"op":"clip_set_loop","clip":"walk","looped":true}"#,
            },
            OpDoc {
                name: "clip_delete",
                what: "Delete a clip.",
                example: r#"{"op":"clip_delete","clip":"walk"}"#,
            },
        ],
    },
    OpFamily {
        title: "Scene",
        ops: &[
            OpDoc {
                name: "object_delete",
                what: "Delete an object plus its LODs, selections, mirror sets and clips.",
                example: r#"{"op":"object_delete","object":"crate"}"#,
            },
            OpDoc {
                name: "object_rename",
                what: "Rename an object; selections, mirror sets, clips and LODs follow.",
                example: r#"{"op":"object_rename","from":"crate","to":"box"}"#,
            },
            OpDoc {
                name: "object_instance",
                what: "Copy an object (mesh + material) with a translation offset.",
                example: r#"{"op":"object_instance","src":"crate","dst":"crate2","translate":[1.5,0.0,0.0]}"#,
            },
            OpDoc {
                name: "lod_generate",
                what: "Generate `<object>_lod1..n` decimated to the given triangle ratios.",
                example: r#"{"op":"lod_generate","object":"crate","ratios":[0.5,0.25]}"#,
            },
            OpDoc {
                name: "set_goal",
                what: "Set the modeling goal; digests and reviews ground against it.",
                example: r#"{"op":"set_goal","goal":"a weathered wooden crate"}"#,
            },
            OpDoc {
                name: "set_class",
                what: "Set the asset class (prop|weapon|building|character|environment); picks the tri budget (environment: 12000 in the classic pack) and which scene rules are Hard.",
                example: r#"{"op":"set_class","class":"environment"}"#,
            },
            OpDoc {
                name: "set_reference",
                what: "Register a project-relative reference image (style target). Critique attaches every reference as \"reference image N\" and scores `reference_match` against it.",
                example: r#"{"op":"set_reference","path":"refs/cave.jpg"}"#,
            },
            OpDoc {
                name: "tag_object",
                what: "Tag an object for the scene gate: `contact` (it may touch/intersect others: stalagmites in a floor), `open` (shell intentionally not closed: cutout planes, a cave mouth). `on:false` removes the tag.",
                example: r#"{"op":"tag_object","object":"stalagmites","tag":"contact"}"#,
            },
        ],
    },
    OpFamily {
        title: "Organic geometry (environments)",
        ops: &[
            OpDoc {
                name: "prim_tunnel",
                what: "Swept tube along a polyline (>=2 points), one ring per point, OPEN ends; `radii` has one entry (constant) or one per point. UVs: u around, v along, one seam column. Passages between caverns.",
                example: r#"{"op":"prim_tunnel","object":"passage","path":[[0.0,1.0,0.0],[2.0,1.2,1.0],[4.0,1.0,3.0]],"radii":[1.0,1.3,1.0],"segments":10}"#,
            },
            OpDoc {
                name: "prim_cavern",
                what: "Ellipsoid (`radii` = x,y,z half-extents) as a UV sphere with `segments` around and `rings` up, faces below `floor_y` removed (open floor), then noise-displaced by `noise` (fraction of the smallest radius) with `seed`. Outward normals; tag `open` if you leave the floor cut, or close it with a floor and `join`.",
                example: r#"{"op":"prim_cavern","object":"shell","radii":[6.0,4.0,7.0],"segments":16,"rings":10,"floor_y":-3.0,"noise":0.15,"seed":7}"#,
            },
            OpDoc {
                name: "subdivide",
                what: "Catmull-Clark (`smooth` true, default) or linear midpoint split, `levels` times (default 1). Quadruples faces per level — check the budget first; one level is usually enough for a low-poly organic look.",
                example: r#"{"op":"subdivide","object":"boulder","levels":1,"smooth":true}"#,
            },
            OpDoc {
                name: "displace_noise",
                what: "Move the selected verts along their normals by `amplitude` * fbm noise(-1..1) sampled at position/`scale`; deterministic per `seed`. Turns smooth blobs into faceted rock.",
                example: r#"{"op":"displace_noise","sel":{"q":"all","object":"boulder","kind":"verts"},"amplitude":0.15,"scale":0.8,"seed":3}"#,
            },
            OpDoc {
                name: "smooth",
                what: "Laplacian relax of the selected verts toward their neighbour centroid, `iterations` (default 1) times by `factor` (0..1, default 0.5); boundary verts stay put.",
                example: r#"{"op":"smooth","sel":{"q":"all","object":"boulder","kind":"verts"},"iterations":2,"factor":0.5}"#,
            },
            OpDoc {
                name: "solidify",
                what: "Give an open shell real thickness: offset copy along -normal (`thickness` > 0 = inward), flipped winding, rim quads on every boundary edge. The result is a closed manifold, so the mouth of a cave reads as rock, not a razor edge. Doubles the face count.",
                example: r#"{"op":"solidify","object":"shell","thickness":0.3}"#,
            },
            OpDoc {
                name: "join",
                what: "Merge several objects into one new mesh named `name` (sources are removed; the first source's material is kept). Follow with `merge_verts` to weld coincident verts into one connected surface.",
                example: r#"{"op":"join","objects":["shell","floor","passage"],"name":"cave"}"#,
            },
            OpDoc {
                name: "snap_to_surface",
                what: "Drop the selected verts onto the nearest surface of `target` along `dir` (default -Y, both directions tested). Plants stalagmite bases on an uneven floor; verts with no hit stay put and are listed.",
                example: r#"{"op":"snap_to_surface","sel":{"q":"faces_facing","object":"stalagmites","dir":[0.0,-1.0,0.0]},"target":"cave"}"#,
            },
        ],
    },
    OpFamily {
        title: "Baked lighting (vertex colors)",
        ops: &[
            OpDoc {
                name: "bake_ao",
                what: "Per-vertex ambient occlusion against EVERY object in the scene (`samples` hemisphere rays, default 32), multiplied into the vertex colors by `strength` (default 1). Crevices darken; exported as COLOR_0, multiplied over the unlit texture.",
                example: r#"{"op":"bake_ao","object":"cave","samples":32,"strength":0.9}"#,
            },
            OpDoc {
                name: "bake_sun",
                what: "Half-lambert directional light from `dir` (the direction light travels) tinted by `color` (default warm), multiplied into vertex colors by `strength` (default 0.5). No shadows — AO covers occlusion.",
                example: r#"{"op":"bake_sun","object":"cave","dir":[-0.4,-1.0,0.3],"strength":0.6,"color":[1.0,0.95,0.85]}"#,
            },
            OpDoc {
                name: "bake_glow",
                what: "Additive tint on `object`'s vertices from each emissive `lights` object (its material's emissive * `strength`, fading to zero at `radius` from the light's bounds center). Cheap crystal rim light.",
                example: r#"{"op":"bake_glow","object":"cave","lights":["crystal_a","crystal_b"],"radius":3.0,"strength":0.8}"#,
            },
            OpDoc {
                name: "paint_vertex",
                what: "Blend the selection's vertex colors toward `color` by `strength`; with `facing`, only verts on faces whose normal is within `max_angle_deg` (default 45) of it — moss on upward faces, dirt on downward ones.",
                example: r#"{"op":"paint_vertex","sel":{"q":"all","object":"cave","kind":"faces"},"color":[0.45,0.6,0.3],"strength":0.5,"facing":[0.0,1.0,0.0],"max_angle_deg":40.0}"#,
            },
            OpDoc {
                name: "clear_vertex_colors",
                what: "Remove every vertex color on the object (back to white) so bakes can be redone from scratch.",
                example: r#"{"op":"clear_vertex_colors","object":"cave"}"#,
            },
        ],
    },
];

/// Render SKILL.md. Everything op-shaped comes from `OP_FAMILIES`.
pub fn skill_md() -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str(HEADER);
    for family in OP_FAMILIES {
        out.push_str(&format!("\n### {}\n\n", family.title));
        for op in family.ops {
            out.push_str(&format!("- `{}` — {}\n\n  ```json\n  {}\n  ```\n\n", op.name, op.what, op.example));
        }
    }
    out.push_str(FOOTER);
    out
}

const HEADER: &str = r#"# Degen Modeler — agent skill

Degen Modeler builds low-poly glTF assets from a closed vocabulary of JSON
ops. You never touch vertices with a cursor: you POST ops, read digests,
render contact sheets, and iterate until the gate passes and the review
scores are good.

## Ground rules

- Units are meters, +Y is up, faces are CCW seen from outside.
- Element ids are stable and never reused: `v3` (vert), `f12` (face),
  `e4:9` (edge between v4 and v9). Errors and diffs speak this language.
- Every applied op appends to `ops.jsonl` and bumps `revision`. Replaying
  the ledger reproduces the model byte-for-byte, so never edit files by hand.
- Ops that would breach the pack's triangle budget fail closed (409); the op
  is not applied. Check `budget` in the scene digest before big topology ops.
- The style pack (`GET /pack`) is law: budgets, texel-density band, trim
  sheets + regions, rig presets, clip templates, palette.

## Op wire shape

`POST /op` with `{"op":"<name>", ...params}`. Unknown fields are rejected
loudly. Where an op takes `sel`, pass either a saved selection name
(`"top_faces"`) or an inline query object:

```json
{"q":"faces_facing","object":"crate","dir":[0.0,1.0,0.0],"max_angle_deg":45.0}
```

Query kinds: `all` (kind: verts|faces|edges), `verts` (ids), `faces` (ids),
`edges` (pairs of vert ids), `faces_facing` (dir, max_angle_deg),
`boundary_loop`, `island_of` (face), `by_material` (material), `in_box`
(min, max, kind).

## Op vocabulary
"#;

const FOOTER: &str = r#"## API (127.0.0.1:7799 by default; localhost only)

```
GET  /health                    { ok, name, revision, class }
GET  /scene                     scene digest (+ extra.metrics, extra.references, extra.tags)
GET  /pack                      pack summary: budgets, trims+regions, rigs, clips, palette
GET  /ledger?from=N             ledger lines from revision N
GET  /object/{name}             object digest + uv island count + trim region usage
POST /op        {op json}       apply one op -> outcome {revision, diff, findings}
POST /ops       [op json]       apply a batch; stops at first error, reports index
POST /review    {"tier":"metrics"|"jev"|"vision"}   review report (default jev)
POST /critique  {}              art-direction critique by the vision model (needs OPENAI_API_KEY); sees renders + doc.references (+ interior sheet for building/environment); suggestions name ops
POST /export    {"out":"path"?} gate + write GLB; 409 with the report on hard findings
GET  /render/{kind}?object=&px= PNG (sheet|wireframe|uv|heatmap|filmstrip|interior), X-Artifact header
GET  /artifacts/{file}          fetch a written artifact
POST /raycast   {object, origin, dir}   {face, distance, point} or null
```

Errors are `{error, detail?}`: 400 bad op JSON/params, 404 unknown element,
409 budget or export gate, 502 review upstream.

## The review loop

1. `set_goal`, then block out proportions with primitives; check silhouette
   with `GET /render/sheet` (the artifact name comes back in `X-Artifact`).
2. Add topology only where the silhouette needs it; watch `budget.used_tris`
   in `GET /scene`.
3. UV: primitives already carry meter-scaled UVs with fan/cap seams — go
   straight to `uv_assign_trim` (pack regions) or `uv_assign_rect` (owned
   atlas). `GET /render/uv?object=X` and `GET /render/heatmap` show waste
   and density. `mark_seams`/`uv_unwrap`/`uv_project` are for geometry you
   changed after creation.
4. Materials from pack trims; rig/animate with pack presets when the class
   calls for it (`rig_apply` -> `rig_auto_weights` -> `clip_apply`).
5. `POST /review` after meaningful changes. Findings are dotted rules
   (`mesh.*`, `uv.*`, `rig.*`, `anim.*`, `budget.*`); Hard ones block export.
   Heads (0-1): silhouette, style, seams, waste, done — treat < 0.6 as a
   to-do list, and stop when `done` is high and the gate passes.
6. `POST /critique` before calling it finished: a vision model judges the
   renders like an art director and answers with actions in this op
   vocabulary. Then `POST /export`.

Iterate in small steps: one op, read the outcome diff, re-render when shape
changed. The ledger is your undo-free audit trail — `dgm replay` proves it.

## One-shot playbook (learned the hard way; follow it and the gate stays green)

**Order of operations.** goal -> primitives -> materials BOUND EARLY (density
rules only run with a texture size) -> UV layout -> deform/detail -> review ->
critique -> export. Binding materials late hides band violations until the end.

**Owned atlas layout** (one `File` texture shared by several objects):
partition [0,1]^2 up front — e.g. bark `[0.02,0.02,0.48,0.98]`, foliage
`[0.52,0.02,0.98,0.98]` — and `uv_assign_rect` each object's islands into its
region, always passing `texels_per_meter` (the pack band midpoint, e.g. 275
for band 100..450) so rect-filling can never leave the band. Leave a >=8px
gutter between regions; paint strictly inside them (erase spill).
Cross-object stacking on one region is fine; per-object `uv.waste` warns on
shared atlases are expected — read them per atlas, not per object.

**Revolved shapes** (`prim_lathe`/`prim_cylinder`): side bands, apex fans and
caps are separate pre-seamed islands with sane UVs. Assign the side band to
your main region and each fan/cap to a small patch — never cylindrical-project
a fan (its UV area collapses; density explodes or vanishes). If two islands
must stack, `uv_declare_mirror` says it is intentional.

**Cutout foliage/cloth**: crossed `prim_plane` quads, one `material_new` with
`"alpha":"mask","double_sided":true`, all quads `uv_assign_rect` onto the same
painted region (declare the stack with `uv_declare_mirror` if they share an
object). Paint the region with a mostly-opaque core and a broken rim — soft
alpha edges disappear at the 0.5 cutoff.

**Density fixes**: prefer re-assigning the island (`uv_assign_rect` with
`texels_per_meter`) over `uv_set_texel_density` on face subsets — scaling
faces inside an island distorts neighbours and can push UVs out of [0,1].

**Textures from outside** (degen-paint, DMS): render `GET /render/uv` first
and paint over that layout; drop the PNG at the `File` path; the gate then
measures real density. degen-paint edits are journaled ops — brush passes are
cheap, iterate there instead of settling for a flat fill.

**Budgets**: silhouette spends triangles, texture paints detail. A prop reads
at 100-800 tris; if the silhouette needs nothing more, stop modeling.

## Environments playbook (caves, rooms, ruins — one connected space)

`set_class environment` first: the budget is 12000 tris and the scene gate
turns Hard for `scene.intersects` (objects crossing without a `contact`
tag), `mesh.open_boundary` (open shells without an `open` tag) and
`mesh.inverted`; `scene.floating` warns about anything not touching the rest.
The reference image is the brief: `set_reference` before modeling so every
critique judges against it (`reference_match` head).

**Order.** `prim_cavern` (the room; `floor_y` opens the floor) + `prim_tunnel`
(passages, open ends meet the cavern) -> `solidify` each shell so the mouth
has thickness -> `join` them + `merge_verts` on the seam -> formations
(`prim_lathe` cones, `prim_box` boulders) with `subdivide` 1 ->
`displace_noise` -> `smooth` for faceted rock -> `snap_to_surface` their
bases onto the joined mesh and `tag_object contact` -> materials (wall /
floor / moss sheets, an emissive `material_new` for crystals) -> bakes ->
interior render -> critique -> export.

**Light is baked, not hoped for.** `bake_ao` on every object (against the
whole scene), then `bake_sun` from the mouth direction, then `bake_glow`
with the crystal objects as `lights`; `paint_vertex` with `facing [0,1,0]`
tints upward faces mossy, `facing [0,-1,0]` darkens undersides. The renderer,
viewer and GLB (`COLOR_0`) all multiply these into the unlit texture;
`extra.metrics.lit_range` tells you the value spread you achieved (a flat
scene sits near 1.0/1.0).

**Look from inside.** `GET /render/interior` renders four views from inside
the bounds (mouth looking in, center looking each way, looking up) — the
turntable sheet cannot show a cave. `extra.metrics.connectivity` is the share
of objects touching another (aim for 1.0 after snapping), `repetition` how
many copies of one sheet cover its surface (log-scaled: 1 tile = 0, 64 = 1;
high = visible wallpaper — break it with a second sheet, `paint_vertex` tints,
baked AO/sun, or a lower `texels_per_meter`).
`POST /critique` then sees exterior + interior sheets + the references and
asks about continuity, scale, lighting and focal points.
"#;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use dgm_scene::op::Op;

    use super::*;

    fn op_names() -> impl Iterator<Item = &'static str> {
        OP_FAMILIES.iter().flat_map(|f| f.ops.iter()).map(|o| o.name)
    }

    /// The full wire-name list straight from serde: deserializing an unknown
    /// variant makes serde enumerate every real one.
    fn serde_op_names() -> BTreeSet<String> {
        let err = serde_json::from_value::<Op>(serde_json::json!({ "op": "__nope__" }))
            .expect_err("unknown variant must fail")
            .to_string();
        let (_, list) = err.split_once("expected one of ").expect("serde lists variants");
        list.split(", ")
            .map(|s| s.trim().trim_end_matches('.').trim_matches('`').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn skill_covers_every_op_exactly() {
        let documented: BTreeSet<String> = op_names().map(str::to_string).collect();
        let actual = serde_op_names();
        assert_eq!(documented, actual, "SKILL op table drifted from dgm_scene::op::Op");
        // No duplicate docs.
        assert_eq!(op_names().count(), actual.len());
    }

    #[test]
    fn every_example_parses_as_its_op() {
        for family in OP_FAMILIES {
            for doc in family.ops {
                let op: Op = serde_json::from_str(doc.example)
                    .unwrap_or_else(|e| panic!("example for `{}` invalid: {e}\n{}", doc.name, doc.example));
                assert_eq!(op.name(), doc.name, "example under wrong heading: {}", doc.example);
            }
        }
    }

    #[test]
    fn skill_text_mentions_every_op_and_endpoint() {
        let text = skill_md();
        for name in op_names() {
            assert!(text.contains(&format!("`{name}`")), "SKILL.md missing op `{name}`");
            assert!(
                text.contains(&format!("\"op\":\"{name}\"")),
                "SKILL.md missing example for `{name}`"
            );
        }
        for endpoint in
            ["/health", "/scene", "/pack", "/ledger", "/object/", "/op", "/ops", "/review",
             "/export", "/render/", "/artifacts/", "/raycast"]
        {
            assert!(text.contains(endpoint), "SKILL.md missing endpoint {endpoint}");
        }
    }
}
