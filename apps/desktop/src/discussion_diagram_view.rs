use std::{sync::Arc, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dioxus::prelude::*;
use discussion_diagram::{ProjectionError, MAX_SVG_BYTES};
use recorder_domain::SessionId;
use serde::Deserialize;

use crate::app_state::AppState;

#[derive(Deserialize)]
#[serde(untagged)]
enum RenderResult {
    Success {
        svg: String,
        width: u32,
        warning: bool,
    },
    Failure {
        error: String,
    },
}

struct Image {
    svg: String,
    url: String,
    generation: u64,
    width: u32,
    rev: u64,
    warning: bool,
}

async fn render(diagram: discussion_diagram::Diagram) -> Result<(String, u32, bool), &'static str> {
    async fn run(
        diagram: discussion_diagram::Diagram,
    ) -> Result<(String, u32, bool), &'static str> {
        let ready = document::eval("return typeof globalThis.__meetingramDiagram === 'function';")
            .join::<bool>()
            .await
            .map_err(|_| "描画機能を起動できませんでした")?;
        if !ready {
            document::eval(concat!(
                include_str!("../diagram-renderer/renderer.js"),
                "\nreturn true;"
            ))
            .join::<bool>()
            .await
            .map_err(|_| "描画機能を起動できませんでした")?;
        }
        let eval =
            document::eval("return await globalThis.__meetingramDiagram(await dioxus.recv());");
        eval.send(diagram)
            .map_err(|_| "描画データを渡せませんでした")?;
        match eval.join::<RenderResult>().await {
            Ok(RenderResult::Success {
                svg,
                width,
                warning,
            }) if svg.len() <= MAX_SVG_BYTES && (1..=100_000).contains(&width) => {
                Ok((svg, width, warning))
            }
            Ok(RenderResult::Failure { error }) if error == "limit_exceeded" => {
                Err("図が上限を超えました。議題を選んでください")
            }
            _ => Err("議論を図解できませんでした"),
        }
    }
    tokio::time::timeout(Duration::from_secs(15), run(diagram))
        .await
        .map_err(|_| "描画がタイムアウトしました")?
}

async fn pick_save_path(session_id: SessionId) -> Option<std::path::PathBuf> {
    #[cfg(feature = "e2e-scripted")]
    if crate::scripted_stt_active() {
        if let Some(directory) = std::env::var_os("MEETINGRAM_E2E_TOUR_DIR") {
            return Some(std::path::PathBuf::from(directory).join("discussion-map.svg"));
        }
    }
    rfd::AsyncFileDialog::new()
        .set_file_name(format!("discussion-{session_id}.svg"))
        .add_filter("SVG", &["svg"])
        .save_file()
        .await
        .map(|file| file.path().to_path_buf())
}

#[component]
pub(super) fn DiscussionDiagram(session_id: SessionId, options: Vec<(String, String)>) -> Element {
    let state = use_context::<Arc<AppState>>();
    let mut agenda = use_signal(String::new);
    let mut image = use_signal(|| None::<Image>);
    let mut message = use_signal(|| None::<&'static str>);
    let mut running = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut zoom = use_signal(|| 0_u32);
    let consumer = state.graph_consumer.clone();
    let mut graph_status = use_signal(|| {
        let status = consumer.status(session_id);
        (status.graph_generation, status.rev, status.rebuilding)
    });
    use_future({
        let consumer = consumer.clone();
        move || {
            let consumer = consumer.clone();
            async move {
                loop {
                    let status = consumer.status(session_id);
                    let current = (status.graph_generation, status.rev, status.rebuilding);
                    if current != *graph_status.peek() {
                        graph_status.set(current);
                        if image
                            .peek()
                            .as_ref()
                            .is_some_and(|old| old.generation != current.0 || current.2)
                        {
                            image.set(None);
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    });
    let (generation, rev, rebuilding) = graph_status();
    let has_image = image.read().is_some();
    let zoom_percent = zoom();
    let draw = move |_| {
        if *running.peek() {
            return;
        }
        let scope = agenda();
        let consumer = consumer.clone();
        running.set(true);
        message.set(None);
        spawn(async move {
            let result = async {
                let before = consumer.status(session_id);
                let graph = consumer
                    .load_graph(session_id)
                    .await
                    .map_err(|_| "議論グラフを読み込めませんでした")?;
                let after = consumer.status(session_id);
                if before.rebuilding
                    || after.rebuilding
                    || before.graph_generation != after.graph_generation
                {
                    return Err("再構築が終わってから図解してください");
                }
                let graph_rev = graph.rev;
                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                let threshold = consumer.review_threshold();
                let data = tokio::task::spawn_blocking(move || {
                    discussion_diagram::project(
                        &graph,
                        (!scope.is_empty()).then_some(scope.as_str()),
                        threshold,
                        format!("{timestamp} · 版 {graph_rev}"),
                    )
                })
                .await
                .map_err(|_| "議論グラフを変換できませんでした")?
                .map_err(|error| match error {
                    ProjectionError::Empty => "図解できる議論がまだありません",
                    ProjectionError::UnknownAgenda => "議題を選び直してください",
                    ProjectionError::Limit => "図が上限を超えました。議題を選んでください",
                })?;
                let (svg, width, warning) = render(data).await?;
                let (svg, url) = tokio::task::spawn_blocking(move || {
                    if !discussion_diagram::valid_svg(&svg) {
                        return Err("図の形式を確認できませんでした");
                    }
                    let url = format!(
                        "data:image/svg+xml;base64,{}",
                        STANDARD.encode(svg.as_bytes())
                    );
                    Ok((svg, url))
                })
                .await
                .map_err(|_| "図の形式を確認できませんでした")??;
                let current = consumer.status(session_id);
                if current.rebuilding || current.graph_generation != after.graph_generation {
                    return Err("議論グラフが作り直されました。もう一度図解してください");
                }
                Ok(Image {
                    svg,
                    url,
                    warning,
                    width,
                    generation: after.graph_generation,
                    rev: graph_rev,
                })
            }
            .await;
            match result {
                Ok(rendered) => image.set(Some(rendered)),
                Err(error) => message.set(Some(error)),
            }
            running.set(false);
        });
    };

    rsx! {
        section { aria_label: "議論マップ", style: "width: 100%; min-width: 0;",
            label {
                "図解する範囲"
                select {
                    value: agenda(),
                    disabled: running(),
                    onchange: move |event| {
                        agenda.set(event.value());
                        image.set(None);
                        message.set(None);
                    },
                    option { value: "", "全体" }
                    for (id, title) in options {
                        option { key: "{id}", value: "{id}", "{title}" }
                    }
                }
            }
            button { id: "meetingram-draw-discussion-map", disabled: running() || rebuilding, onclick: draw,
                if running() { "図解中…" } else if has_image { "図を更新" } else { "議論を図解" }
            }
            if let Some(text) = message() {
                p { role: "status", "{text}" }
            }
            if let Some(rendered) = image.read().as_ref().filter(|item| item.generation == generation && !rebuilding) {
                if rev > rendered.rev {
                    p { role: "status", "描画後に議論が更新されました" }
                }
                if rendered.warning {
                    p { "図の一部が読みづらい場合は、議題を選んで表示範囲を絞ってください" }
                }
                label {
                    "表示倍率"
                    select {
                        value: "{zoom_percent}",
                        onchange: move |event| {
                            if let Ok(value) = event.value().parse::<u32>() {
                                zoom.set(value);
                            }
                        },
                        option { value: "0", "全体表示" }
                        for percent in [50, 75, 100, 125, 150, 200] {
                            option { key: "{percent}", value: "{percent}", "{percent}%" }
                        }
                    }
                }
                button { disabled: saving(), onclick: move |_| {
                    if *saving.peek() { return; }
                    let Some(svg) = image.peek().as_ref().map(|item| item.svg.clone()) else { return };
                    saving.set(true);
                    spawn(async move {
                        if let Some(path) = pick_save_path(session_id).await {
                            let result = tokio::task::spawn_blocking(move || std::fs::write(path, svg)).await;
                            message.set(Some(if matches!(result, Ok(Ok(()))) { "SVG を保存しました" } else { "SVG を保存できませんでした" }));
                        }
                        saving.set(false);
                    });
                }, "SVG を保存" }
                div { style: "overflow: auto; width: 100%; max-height: 65vh;",
                    img { id: "meetingram-discussion-map", alt: "会議の議論マップ", src: "{rendered.url}", style: if zoom_percent == 0 { "width: 100%;".to_owned() } else { format!("width: {}px; max-width: none;", rendered.width * zoom_percent / 100) } }
                }
            }
        }
    }
}
