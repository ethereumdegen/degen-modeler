//! bevy_egui panels: digest (left), op ticker (bottom), review + clips +
//! engine preview + op console (right).

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{self, Color32, LayerId, RichText, UiBuilder};
use dgm_scene::Op;

use crate::feed::ReviewView;
use crate::state::{
    ActiveClip, Console, DigestCache, PickState, Playback, Preview, Review, Ticker, UiProject,
    ledger_mtime,
};

const HARD_RED: Color32 = Color32::from_rgb(230, 80, 70);
const WARN_YELLOW: Color32 = Color32::from_rgb(220, 180, 60);
const PASS_GREEN: Color32 = Color32::from_rgb(90, 200, 110);

#[expect(clippy::too_many_arguments, reason = "one egui pass over all panel state")]
pub fn panels(
    mut contexts: EguiContexts,
    mut up: ResMut<UiProject>,
    digest: Res<DigestCache>,
    ticker: Res<Ticker>,
    review: Res<Review>,
    mut console: ResMut<Console>,
    mut playback: ResMut<Playback>,
    mut preview: ResMut<Preview>,
    pick: Res<PickState>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let mut root = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("dgm_root"),
        UiBuilder::new()
            .layer_id(LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::left("dgm_digest")
        .resizable(true)
        .default_size(230.0)
        .show(&mut root, |ui| digest_panel(ui, &up, &digest, &pick));

    egui::Panel::right("dgm_review")
        .resizable(true)
        .default_size(320.0)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                review_section(ui, review.view.as_ref());
                ui.separator();
                clips_section(ui, &up, &mut playback);
                ui.separator();
                preview_section(ui, &mut preview);
                ui.separator();
                console_section(ui, &mut up, &mut console);
            });
        });

    egui::Panel::bottom("dgm_ticker")
        .resizable(true)
        .default_size(110.0)
        .show(&mut root, |ui| ticker_panel(ui, &ticker));

    Ok(())
}

fn digest_panel(ui: &mut egui::Ui, up: &UiProject, digest: &DigestCache, pick: &PickState) {
    ui.heading(&up.project.meta.name);
    ui.label(format!("rev {} · {}", digest.revision, digest.class));
    if let Some(goal) = &up.project.meta.goal {
        ui.small(goal);
    }
    if let Some(status) = &up.status {
        ui.colored_label(HARD_RED, status);
    }
    ui.separator();

    let (badge, color) = if digest.hard > 0 {
        (format!("GATE FAIL · {} hard", digest.hard), HARD_RED)
    } else if digest.warn > 0 {
        (format!("GATE PASS · {} warn", digest.warn), WARN_YELLOW)
    } else {
        ("GATE PASS".to_string(), PASS_GREEN)
    };
    ui.label(RichText::new(badge).color(color).strong());
    for message in &digest.hard_messages {
        ui.small(RichText::new(message).color(HARD_RED));
    }
    ui.separator();

    match digest.budget {
        Some(budget) => {
            let over = digest.total_tris > budget;
            let color = if over { HARD_RED } else { PASS_GREEN };
            ui.colored_label(color, format!("tris {} / {}", digest.total_tris, budget));
            ui.add(egui::ProgressBar::new(digest.total_tris as f32 / budget.max(1) as f32));
        }
        None => {
            ui.label(format!("tris {} (no budget for class)", digest.total_tris));
        }
    }
    ui.separator();

    ui.strong("objects");
    if digest.rows.is_empty() {
        ui.small("empty scene — apply a prim_* op");
    }
    for row in &digest.rows {
        let mat = row.material.as_deref().unwrap_or("no material");
        let lod = if row.is_lod { " · lod" } else { "" };
        ui.label(format!("{} · {} tris · {mat}{lod}", row.name, row.tris));
    }
    ui.separator();
    ui.label(format!("picked: {}", pick.text.as_deref().unwrap_or("(click a face)")));
}

fn ticker_panel(ui: &mut egui::Ui, ticker: &Ticker) {
    ui.strong("op ticker");
    egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
        if ticker.lines.is_empty() {
            ui.small("ledger is empty");
        }
        for line in &ticker.lines {
            ui.monospace(format!("r{:<5} {:<6} {}", line.rev, line.actor, line.op));
        }
    });
}

fn review_section(ui: &mut egui::Ui, review: Option<&ReviewView>) {
    ui.heading("review");
    let Some(view) = review else {
        ui.small("no artifacts/r*-review.json yet");
        return;
    };
    ui.small(&view.file);
    for (name, value) in &view.heads {
        ui.add(egui::ProgressBar::new(*value).text(format!("{name} {value:.2}")));
    }
    if !view.findings.is_empty() {
        ui.add_space(4.0);
        ui.strong("findings");
    }
    for finding in &view.findings {
        let color = if finding.severity == "hard" { HARD_RED } else { WARN_YELLOW };
        ui.label(
            RichText::new(format!("{}: {}", finding.rule, finding.message)).color(color).small(),
        );
    }
    for note in &view.notes {
        ui.small(note);
    }
}

fn clips_section(ui: &mut egui::Ui, up: &UiProject, playback: &mut Playback) {
    ui.heading("clips");
    let clips: Vec<String> = up.project.doc.clips.keys().cloned().collect();
    if clips.is_empty() {
        ui.small("no clips — apply clip_apply");
    }
    for name in clips {
        let active = playback.active.as_ref().is_some_and(|a| a.clip == name);
        if ui.selectable_label(active, format!("Play {name}")).clicked() {
            playback.active = Some(ActiveClip { clip: name, t: 0.0 });
        }
    }
    if playback.active.is_some() && ui.button("Stop").clicked() {
        playback.active = None;
        playback.restore = true;
    }
}

fn preview_section(ui: &mut egui::Ui, preview: &mut Preview) {
    ui.heading("engine preview");
    let label = if preview.entity.is_some() { "Refresh engine preview" } else { "Engine preview" };
    if ui.button(label).clicked() {
        preview.requested = true;
    }
    if let Some(status) = &preview.status {
        ui.small(status);
    }
}

fn console_section(ui: &mut egui::Ui, up: &mut UiProject, console: &mut Console) {
    ui.heading("op console");
    ui.add(
        egui::TextEdit::multiline(&mut console.input)
            .code_editor()
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .hint_text(r#"{"op": "prim_box", "object": "crate", "size": [1, 1, 1]}"#),
    );
    if ui.button("Apply (actor: ui)").clicked() {
        apply_console(up, console);
    }
    if let Some(output) = &console.output {
        ui.monospace(output);
    }
}

fn apply_console(up: &mut UiProject, console: &mut Console) {
    let op: Op = match serde_json::from_str(console.input.trim()) {
        Ok(op) => op,
        Err(e) => {
            console.output = Some(format!("bad op JSON: {e}"));
            return;
        }
    };
    match up.project.apply(op, "ui") {
        Ok(outcome) => {
            console.output = Some(
                serde_json::to_string_pretty(&outcome)
                    .unwrap_or_else(|_| format!("applied rev {}", outcome.revision)),
            );
            // The apply already updated the in-memory doc; refresh the mtime
            // cache so the next poll doesn't reload it a second time.
            up.dirty = true;
            up.ledger_mtime = ledger_mtime(&up.project);
        }
        Err(e) => console.output = Some(format!("error: {e}")),
    }
}
