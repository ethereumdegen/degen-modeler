//! `dgm` — the Degen Modeler CLI. Thin verbs over `dgm_scene::Project` and
//! the shared `dgm_api::orchestrate` pipeline, so CLI and API behave
//! identically. Actor string for ledger provenance: `cli`.

mod skill;

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use dgm_api::orchestrate::{self, RenderKind, StageError};
use dgm_atlas::AssetClass;
use dgm_scene::op::Op as SceneOp;
use dgm_scene::project::Project;
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "dgm", version, about = "Degen Modeler: agent-first low-poly glTF modeling")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Create a project directory with a self-contained pack copy.
    New {
        dir: PathBuf,
        /// Asset name; defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
        /// prop | weapon | building | character
        #[arg(long, default_value = "prop")]
        class: String,
        #[arg(long)]
        goal: Option<String>,
        /// Pack source dir; default $DGM_PACK, then the bundled classic pack.
        #[arg(long)]
        pack: Option<PathBuf>,
    },
    /// Serve the agent API on 127.0.0.1.
    Serve {
        dir: PathBuf,
        #[arg(long, default_value_t = dgm_api::DEFAULT_PORT)]
        port: u16,
    },
    /// Apply one op: a JSON argument, or `-` to read it from stdin.
    Op { dir: PathBuf, json: String },
    /// Run the deterministic gate; exit 1 on hard findings.
    Check { dir: PathBuf },
    /// Render + measure + review; writes artifacts/r<rev>-review.json.
    Review {
        dir: PathBuf,
        /// metrics | jev | vision
        #[arg(long, default_value = "jev")]
        tier: String,
    },
    /// Art-direction critique by the configured vision model (needs
    /// OPENAI_API_KEY or ANTHROPIC_API_KEY); writes artifacts/r<rev>-critique.json.
    Critique {
        dir: PathBuf,
        /// Print the raw report JSON instead of the readable form.
        #[arg(long)]
        json: bool,
    },
    /// Render one artifact: sheet|wireframe|uv|heatmap|filmstrip.
    Render {
        dir: PathBuf,
        #[arg(long)]
        kind: String,
        /// Extra copy of the PNG (the artifact is always written).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Object name; required for kind `uv`.
        #[arg(long)]
        object: Option<String>,
        #[arg(long, default_value_t = orchestrate::DEFAULT_VIEW_PX)]
        px: u32,
    },
    /// Gate + export exports/model.glb (or --out); exit 1 on hard findings.
    Export {
        dir: PathBuf,
        #[arg(long)]
        out: Option<String>,
    },
    /// Law check: replay the ledger, compare export bytes; exit 1 on drift.
    Replay { dir: PathBuf },
    /// Print the scene digest (metrics merged in when available).
    Digest { dir: PathBuf },
    /// Print SKILL.md (op vocabulary + API) or install it with --install.
    Skill {
        #[arg(long)]
        install: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::New { dir, name, class, goal, pack } => cmd_new(&dir, name, &class, goal, pack),
        Cmd::Serve { dir, port } => cmd_serve(&dir, port).await,
        Cmd::Op { dir, json } => cmd_op(&dir, &json),
        Cmd::Check { dir } => cmd_check(&dir),
        Cmd::Review { dir, tier } => cmd_review(&dir, &tier).await,
        Cmd::Critique { dir, json } => cmd_critique(&dir, json).await,
        Cmd::Render { dir, kind, out, object, px } => cmd_render(&dir, &kind, out, object, px),
        Cmd::Export { dir, out } => cmd_export(&dir, out),
        Cmd::Replay { dir } => cmd_replay(&dir),
        Cmd::Digest { dir } => cmd_digest(&dir),
        Cmd::Skill { install } => cmd_skill(install),
    }
}

fn parse_class(s: &str) -> Result<AssetClass> {
    serde_json::from_value(serde_json::Value::String(s.into()))
        .map_err(|_| anyhow!("unknown class `{s}`; use prop|weapon|building|character"))
}

/// Pack source: --pack, else $DGM_PACK, else the repo's `packs/classic`
/// (compile-time manifest dir), else `packs/classic` near the executable.
fn resolve_pack(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p);
    } else if let Ok(env) = std::env::var("DGM_PACK") {
        candidates.push(PathBuf::from(env));
    } else {
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/classic"));
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join("packs/classic"));
            candidates.push(dir.join("../packs/classic"));
            candidates.push(dir.join("../../packs/classic"));
        }
    }
    candidates
        .iter()
        .find(|c| c.join("pack.json").is_file())
        .cloned()
        .ok_or_else(|| anyhow!("no pack.json found; tried {candidates:?} (use --pack or $DGM_PACK)"))
}

fn cmd_new(
    dir: &Path,
    name: Option<String>,
    class: &str,
    goal: Option<String>,
    pack: Option<PathBuf>,
) -> Result<()> {
    let class = parse_class(class)?;
    let pack_src = resolve_pack(pack)?;
    let name = match name {
        Some(n) => n,
        None => dir
            .file_name()
            .and_then(|s| s.to_str())
            .context("cannot infer --name from the directory path")?
            .to_string(),
    };
    let project = Project::init(dir, &pack_src, &name, class, goal)?;
    println!(
        "{}",
        json!({ "root": project.root, "name": project.meta.name, "class": class, "pack": pack_src })
    );
    Ok(())
}

async fn cmd_serve(dir: &Path, port: u16) -> Result<()> {
    let project = Project::load(dir)?;
    eprintln!("dgm api: http://127.0.0.1:{port} ({})", project.meta.name);
    dgm_api::serve(project, port).await?;
    Ok(())
}

fn cmd_op(dir: &Path, json: &str) -> Result<()> {
    let text = if json == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        json.to_string()
    };
    let op: SceneOp = serde_json::from_str(&text).context("bad op json")?;
    let mut project = Project::load(dir)?;
    match project.apply(op, "cli") {
        Ok(outcome) => {
            println!("{}", serde_json::to_string_pretty(&outcome)?);
            Ok(())
        }
        Err(e) => {
            eprintln!("op failed: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_check(dir: &Path) -> Result<()> {
    let project = Project::load(dir)?;
    let report = dgm_jev::gate(&project.doc, &project.pack);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.pass {
        std::process::exit(1);
    }
    Ok(())
}

async fn cmd_review(dir: &Path, tier: &str) -> Result<()> {
    let tier = match tier {
        "metrics" => dgm_jev::Tier::Metrics,
        "jev" => dgm_jev::Tier::Jev,
        "vision" => dgm_jev::Tier::Vision,
        other => bail!("unknown tier `{other}`; use metrics|jev|vision"),
    };
    let project = Project::load(dir)?;
    let report = orchestrate::review_project(&project, tier).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn cmd_critique(dir: &Path, raw: bool) -> Result<()> {
    let project = Project::load(dir)?;
    let report = orchestrate::critique_project(&project).await?;
    if raw {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let reference = report
        .reference_match
        .map(|r| format!(" · reference_match {r:.2}"))
        .unwrap_or_default();
    println!("critique · {} · style_fit {:.2}{reference} · {:?} USD · {}ms", report.model,
        report.style_fit, report.cost_usd.unwrap_or(0.0), report.latency_ms);
    println!("\nVERDICT\n  {}", report.verdict);
    for n in &report.notes {
        println!("  note: {n}");
    }
    if !report.strengths.is_empty() {
        println!("\nSTRENGTHS");
        for s in &report.strengths {
            println!("  + {s}");
        }
    }
    if !report.issues.is_empty() {
        println!("\nISSUES");
        for i in &report.issues {
            println!("  ! [{}] {} — {}", i.severity, i.what, i.where_);
        }
    }
    if !report.suggestions.is_empty() {
        println!("\nSUGGESTIONS");
        for (n, s) in report.suggestions.iter().enumerate() {
            println!("  {}. {} (ops: {}) — {}", n + 1, s.action, s.ops.join(", "), s.impact);
        }
    }
    Ok(())
}

fn cmd_render(
    dir: &Path,
    kind: &str,
    out: Option<PathBuf>,
    object: Option<String>,
    px: u32,
) -> Result<()> {
    let kind = RenderKind::parse(kind)
        .ok_or_else(|| anyhow!("unknown render kind `{kind}`; use sheet|wireframe|uv|heatmap|filmstrip|interior"))?;
    let project = Project::load(dir)?;
    let rendered = orchestrate::render_project(&project, kind, object.as_deref(), px)?;
    let path = match out {
        Some(p) => {
            std::fs::write(&p, &rendered.png).with_context(|| format!("writing {}", p.display()))?;
            p
        }
        None => project.artifacts_dir().join(&rendered.artifact),
    };
    println!("{}", path.display());
    Ok(())
}

fn cmd_export(dir: &Path, out: Option<String>) -> Result<()> {
    let project = Project::load(dir)?;
    match orchestrate::export_project(&project, out.as_deref()) {
        Ok(o) => {
            println!("{}", json!({ "path": o.path, "bytes": o.bytes, "gate": o.gate }));
            Ok(())
        }
        Err(StageError::GateFailed(report)) => {
            eprintln!("export blocked by the gate:");
            println!("{}", serde_json::to_string_pretty(&report)?);
            std::process::exit(1);
        }
        Err(e) => Err(e.into()),
    }
}

fn cmd_replay(dir: &Path) -> Result<()> {
    let project = Project::load(dir)?;
    let opts =
        dgm_gltf::ExportOptions { base_dir: project.root.clone(), embed_report: None };
    let live = dgm_gltf::export_glb(&project.doc, &project.pack, &opts)?;
    let replayed_doc = project.replay_fresh()?;
    let replayed = dgm_gltf::export_glb(&replayed_doc, &project.pack, &opts)?;
    if live == replayed {
        println!("replay ok: rev {} -> {} bytes, byte-identical", project.doc.revision, live.len());
        Ok(())
    } else {
        eprintln!(
            "REPLAY DRIFT: live export {} bytes != replayed export {} bytes",
            live.len(),
            replayed.len()
        );
        std::process::exit(1);
    }
}

fn cmd_digest(dir: &Path) -> Result<()> {
    let project = Project::load(dir)?;
    println!("{}", serde_json::to_string_pretty(&orchestrate::scene_digest(&project))?);
    Ok(())
}

fn cmd_skill(install: Option<PathBuf>) -> Result<()> {
    let text = skill::skill_md();
    match install {
        Some(path) => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
            println!("{}", path.display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_new_with_defaults_and_overrides() {
        let cli = Cli::try_parse_from(["dgm", "new", "proj"]).unwrap();
        let Cmd::New { dir, name, class, goal, pack } = cli.cmd else { panic!("not New") };
        assert_eq!(dir, PathBuf::from("proj"));
        assert_eq!(name, None);
        assert_eq!(class, "prop");
        assert_eq!(goal, None);
        assert_eq!(pack, None);

        let cli = Cli::try_parse_from([
            "dgm", "new", "proj", "--name", "crate", "--class", "weapon", "--goal", "a sword",
            "--pack", "packs/classic",
        ])
        .unwrap();
        let Cmd::New { name, class, goal, pack, .. } = cli.cmd else { panic!("not New") };
        assert_eq!(name.as_deref(), Some("crate"));
        assert_eq!(class, "weapon");
        assert_eq!(goal.as_deref(), Some("a sword"));
        assert_eq!(pack, Some(PathBuf::from("packs/classic")));
    }

    #[test]
    fn parses_serve_port_default_and_flag() {
        let cli = Cli::try_parse_from(["dgm", "serve", "proj"]).unwrap();
        let Cmd::Serve { port, .. } = cli.cmd else { panic!("not Serve") };
        assert_eq!(port, 7799);

        let cli = Cli::try_parse_from(["dgm", "serve", "proj", "--port", "8080"]).unwrap();
        let Cmd::Serve { port, .. } = cli.cmd else { panic!("not Serve") };
        assert_eq!(port, 8080);
    }

    #[test]
    fn parses_op_stdin_marker_and_render_defaults() {
        let cli = Cli::try_parse_from(["dgm", "op", "proj", "-"]).unwrap();
        let Cmd::Op { json, .. } = cli.cmd else { panic!("not Op") };
        assert_eq!(json, "-");

        let cli = Cli::try_parse_from(["dgm", "render", "proj", "--kind", "sheet"]).unwrap();
        let Cmd::Render { kind, out, object, px, .. } = cli.cmd else { panic!("not Render") };
        assert_eq!(kind, "sheet");
        assert_eq!(out, None);
        assert_eq!(object, None);
        assert_eq!(px, 512);
    }

    #[test]
    fn parses_review_tier_and_skill_install() {
        let cli = Cli::try_parse_from(["dgm", "review", "proj"]).unwrap();
        let Cmd::Review { tier, .. } = cli.cmd else { panic!("not Review") };
        assert_eq!(tier, "jev");

        let cli = Cli::try_parse_from(["dgm", "skill", "--install", "SKILL.md"]).unwrap();
        let Cmd::Skill { install } = cli.cmd else { panic!("not Skill") };
        assert_eq!(install, Some(PathBuf::from("SKILL.md")));

        assert!(Cli::try_parse_from(["dgm", "bogus"]).is_err());
        assert!(Cli::try_parse_from(["dgm", "op", "proj"]).is_err()); // json arg required
    }

    #[test]
    fn class_parses_via_pack_serde_names() {
        assert_eq!(parse_class("prop").unwrap(), AssetClass::Prop);
        assert_eq!(parse_class("character").unwrap(), AssetClass::Character);
        assert!(parse_class("hero").is_err());
    }
}
