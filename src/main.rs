use gpui_kit as gpui;
use gpui_kit::TestSupportExt;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    theme::{Theme, ThemeMode},
};
use gpui_kit::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size, uniform_list,
};
use renderdocx::{
    backend::{self, Settings},
    i18n::Language,
    replay::{Session, Snapshot},
};
use std::path::PathBuf;

struct Workspace {
    settings: Settings,
    status: Option<String>,
    capture: Option<PathBuf>,
    session: Option<Session>,
    snapshot: Option<Snapshot>,
    selected: Option<usize>,
    show_textures: bool,
    selected_texture: Option<usize>,
    open_revision: u64,
    selection_revision: u64,
}

impl Workspace {
    fn t(
        &self,
        en: &'static str,
        zh: &'static str,
        ja: &'static str,
        ko: &'static str,
    ) -> &'static str {
        self.settings.language.text(en, zh, ja, ko)
    }

    fn result(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.status = Some(match result {
            Ok(()) => self.t("Process started. Capture and replay results are reported by RenderDoc.", "进程已启动，捕获与回放结果由 RenderDoc 报告。", "プロセスを起動しました。キャプチャとリプレイの結果は RenderDoc に表示されます。", "프로세스를 시작했습니다. 캡처 및 재생 결과는 RenderDoc에서 확인하세요.").to_owned(),
            Err(e) => format!("{}: {e}", self.t("Could not start", "启动失败", "起動できません", "시작할 수 없음")),
        });
        cx.notify();
    }

    fn save(&mut self) {
        if let Err(error) = self.settings.save() {
            self.status = Some(format!(
                "{}: {error}",
                self.t(
                    "Could not save settings",
                    "无法保存设置",
                    "設定を保存できません",
                    "설정을 저장할 수 없습니다"
                )
            ));
        }
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cap"))
        {
            let result = backend::open_capture(&self.settings, &path);
            self.result(result, cx);
            return;
        }
        self.open_revision += 1;
        let revision = self.open_revision;
        let settings = self.settings.clone();
        let capture = path.clone();
        self.status = Some(
            self.t(
                "Opening capture for native replay…",
                "正在打开捕获并初始化原生回放…",
                "キャプチャを開いてリプレイを初期化中…",
                "캡처를 열고 리플레이를 초기화하는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { Session::open(&settings, &path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != revision {
                    return;
                }
                match result {
                    Ok((session, snapshot)) => {
                        this.capture = Some(capture);
                        this.snapshot = Some(snapshot);
                        this.session = Some(session);
                        this.selected = None;
                        this.selected_texture = None;
                        this.show_textures = false;
                        this.status = None;
                    }
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Could not open capture",
                                "无法打开捕获文件",
                                "キャプチャを開けません",
                                "캡처를 열 수 없습니다"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(action) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.actions.get(index))
        else {
            return;
        };
        let event = action.event_id;
        let revision = session.next_revision();
        self.selection_revision = revision;
        let open_revision = self.open_revision;
        self.status = Some(
            self.t(
                "Replaying event…",
                "正在回放事件…",
                "イベントをリプレイ中…",
                "이벤트를 리플레이하는 중…",
            )
            .to_owned(),
        );
        cx.notify();
        let task = cx
            .background_executor()
            .spawn(async move { session.select(event, revision) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.open_revision != open_revision || this.selection_revision != revision {
                    return;
                }
                match result {
                    Ok(true) => {
                        this.selected = Some(index);
                        this.status = None;
                    }
                    Ok(false) => return,
                    Err(error) => {
                        this.status = Some(format!(
                            "{}: {error}",
                            this.t(
                                "Replay failed",
                                "回放失败",
                                "リプレイに失敗しました",
                                "리플레이 실패"
                            )
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn capture_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self
            .snapshot
            .as_ref()
            .expect("capture view requires a snapshot");
        let count = if self.show_textures {
            snapshot.textures.len()
        } else {
            snapshot.actions.len()
        };
        let api = match snapshot.api {
            0 => "D3D11",
            1 => "D3D12",
            2 => "OpenGL",
            3 => "Vulkan",
            _ => "Unknown",
        };
        let title = self
            .capture
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let detail = self.selected.and_then(|i| snapshot.actions.get(i)).cloned();
        let texture_detail = self
            .selected_texture
            .and_then(|i| snapshot.textures.get(i))
            .cloned();
        let counts = format!(
            "{}  ·  {} {}  ·  {} {}  ·  {} {}",
            api,
            snapshot.resource_count,
            self.t("resources", "资源", "リソース", "리소스"),
            snapshot.texture_count,
            self.t("textures", "纹理", "テクスチャ", "텍스처"),
            snapshot.buffer_count,
            self.t("buffers", "缓冲区", "バッファ", "버퍼")
        );
        let frame = format!(
            "{} {}{}",
            self.t("Frame", "帧", "フレーム", "프레임"),
            snapshot.frame,
            if snapshot.degraded {
                self.t(
                    " · degraded replay",
                    " · 降级回放",
                    " · 機能制限あり",
                    " · 제한된 리플레이",
                )
            } else {
                ""
            }
        );
        let back = self.t("Workspace", "工作台", "ワークスペース", "작업 공간");
        let open = self.t(
            "Open capture",
            "打开捕获文件",
            "キャプチャを開く",
            "캡처 열기",
        );
        let upstream = self.t(
            "Open in RenderDoc",
            "在 RenderDoc 中打开",
            "RenderDoc で開く",
            "RenderDoc에서 열기",
        );
        let events = self.t("EVENTS", "事件", "イベント", "이벤트");
        let textures_label = self.t("Textures", "纹理", "テクスチャ", "텍스처");
        let texture_label = self.t("Texture", "纹理", "テクスチャ", "텍스처");
        let array_label = self.t("Array", "数组", "配列", "배열");
        let samples_label = self.t("Samples", "采样", "サンプル", "샘플");
        let bytes_label = self.t("Bytes", "字节", "バイト", "바이트");
        let format_label = self.t("Format", "格式", "形式", "형식");
        let component_label = self.t("Component", "分量", "成分", "구성 요소");
        let texture_details = self.t(
            "TEXTURE DETAILS",
            "纹理详情",
            "テクスチャの詳細",
            "텍스처 상세",
        );
        let texture_empty = self.t(
            "Select a texture to inspect its metadata.",
            "选择纹理查看元数据。",
            "テクスチャを選択して情報を表示します。",
            "텍스처를 선택해 정보를 확인하세요.",
        );
        let details = self.t("ACTION DETAILS", "操作详情", "アクション詳細", "작업 상세");
        let empty = self.t(
            "Select an action to replay its event.",
            "选择操作以回放对应事件。",
            "アクションを選択してイベントをリプレイします。",
            "작업을 선택하여 이벤트를 리플레이합니다.",
        );
        let event_label = self.t("Event", "事件", "イベント", "이벤트");
        let action_label = self.t("Action", "操作", "アクション", "작업");
        let flags_label = self.t("Flags", "标志", "フラグ", "플래그");
        let indices_label = self.t(
            "Indices / vertices",
            "索引 / 顶点",
            "インデックス / 頂点",
            "인덱스 / 정점",
        );
        let instances_label = self.t("Instances", "实例", "インスタンス", "인스턴스");
        let status = self.status.clone();

        let list = if self.show_textures {
            uniform_list(
                "capture-textures",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let texture = this.snapshot.as_ref()?.textures.get(index)?;
                            let name = if texture.name.is_empty() {
                                format!("{texture_label} {index}")
                            } else {
                                texture.name.clone()
                            };
                            let dimensions = format!(
                                "{} × {} × {}",
                                texture.width, texture.height, texture.depth
                            );
                            Some(
                                div()
                                    .id(SharedString::from(format!("texture-{index}")))
                                    .test_support()
                                    .h(px(32.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if this.selected_texture == Some(index) {
                                        0x304c7c
                                    } else {
                                        0x192332
                                    }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(div().overflow_hidden().child(name))
                                    .child(div().text_color(rgb(0x91a7c0)).child(dimensions))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected_texture = Some(index);
                                        cx.notify();
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        } else {
            uniform_list(
                "capture-actions",
                count,
                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let action = this.snapshot.as_ref()?.actions.get(index)?;
                            let selected = this.selected == Some(index);
                            let name = action.name.clone();
                            let event = action.event_id;
                            let indent = action.depth as f32 * 14.0 + 12.0;
                            Some(
                                div()
                                    .id(index)
                                    .test_support()
                                    .h(px(32.))
                                    .pl(px(indent))
                                    .pr_3()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .cursor_pointer()
                                    .bg(rgb(if selected { 0x304c7c } else { 0x192332 }))
                                    .hover(|row| row.bg(rgb(0x263950)))
                                    .child(
                                        div()
                                            .w(px(64.))
                                            .flex_none()
                                            .text_color(rgb(0x91a7c0))
                                            .child(event.to_string()),
                                    )
                                    .child(div().overflow_hidden().child(name))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.select(index, cx)),
                                    ),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .into_any_element()
        };

        let detail_view = if self.show_textures {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x82aaff))
                        .child(texture_details),
                )
                .when_some(texture_detail, |view, texture| {
                    view.child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(texture.name),
                    )
                    .child(format!(
                        "{} × {} × {}",
                        texture.width, texture.height, texture.depth
                    ))
                    .child(format!(
                        "Mip: {}  ·  {array_label}: {}  ·  {samples_label}: {}",
                        texture.mips, texture.array_size, texture.samples
                    ))
                    .child(format!("{bytes_label}: {}", texture.bytes))
                    .child(format!(
                        "{format_label}: {}  ·  {component_label}: {} / {} × {}",
                        texture.format_type,
                        texture.component_type,
                        texture.component_count,
                        texture.component_byte_width
                    ))
                })
                .when(self.selected_texture.is_none(), |view| {
                    view.child(div().text_color(rgb(0x9eacc0)).child(texture_empty))
                })
        } else {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0x82aaff)).child(details))
                .when_some(detail, |view, action| {
                    view.child(div().font_weight(gpui::FontWeight::BOLD).child(action.name))
                        .child(format!("{event_label}: {}", action.event_id))
                        .child(format!("{action_label}: {}", action.action_id))
                        .child(format!("{flags_label}: 0x{:08X}", action.flags))
                        .child(format!("{indices_label}: {}", action.indices))
                        .child(format!("{instances_label}: {}", action.instances))
                })
                .when(self.selected.is_none(), |view| {
                    view.child(div().text_color(rgb(0x9eacc0)).child(empty))
                })
        };

        div()
            .size_full()
            .bg(rgb(0x101722))
            .text_color(rgb(0xe6edf6))
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "capture-back",
                        back,
                        false,
                        |this, cx| {
                            this.open_revision += 1;
                            this.session = None;
                            this.snapshot = None;
                            this.capture = None;
                            this.selected = None;
                            this.status = None;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(button(
                        "capture-open",
                        open,
                        true,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("RenderDoc", &["rdc", "cap"])
                                .pick_file()
                            {
                                this.open(path, cx);
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "capture-upstream",
                        upstream,
                        false,
                        |this, cx| {
                            if let Some(path) = this.capture.as_ref() {
                                let result = backend::open_capture(&this.settings, path);
                                this.result(result, cx);
                            }
                        },
                        cx,
                    )),
            )
            .child(div().text_sm().text_color(rgb(0x9eacc0)).child(title))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_xl().child(frame))
                    .child(div().text_sm().text_color(rgb(0x9eacc0)).child(counts)),
            )
            .when_some(status, |view, text| {
                view.child(div().text_color(rgb(0xd4b77c)).child(text))
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(0x192332))
                            .rounded_lg()
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .px_3()
                                    .py_2()
                                    .child(button(
                                        "events-tab",
                                        events,
                                        !self.show_textures,
                                        |this, cx| {
                                            this.show_textures = false;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(button(
                                        "textures-tab",
                                        textures_label,
                                        self.show_textures,
                                        |this, cx| {
                                            this.show_textures = true;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .self_center()
                                            .text_sm()
                                            .text_color(rgb(0x91a7c0))
                                            .child(count.to_string()),
                                    ),
                            )
                            .child(list),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .flex_none()
                            .bg(rgb(0x192332))
                            .rounded_lg()
                            .p_4()
                            .child(detail_view),
                    ),
            )
    }
}

fn button(
    id: &'static str,
    title: &'static str,
    accent: bool,
    callback: impl Fn(&mut Workspace, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let button = Button::new(id)
        .label(title)
        .on_click(cx.listener(move |this, _, _, cx| callback(this, cx)));
    if accent {
        button.primary()
    } else {
        button.secondary()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.snapshot.is_some() {
            return self.capture_view(cx).into_any_element();
        }
        let title = self.t(
            "A clearer view of every frame.",
            "让每一帧都清晰可见。",
            "すべてのフレームを、より明確に。",
            "모든 프레임을 더 선명하게.",
        );
        let intro = self.t(
            "A native workspace built around RenderDoc.",
            "基于 RenderDoc 的原生图形调试工作台。",
            "RenderDoc を中心にしたネイティブワークスペース。",
            "RenderDoc 기반의 네이티브 작업 공간입니다.",
        );
        let open = self.t(
            "Open capture",
            "打开捕获文件",
            "キャプチャを開く",
            "캡처 열기",
        );
        let launch = self.t(
            "Launch application",
            "启动应用并捕获",
            "アプリを起動してキャプチャ",
            "앱 실행 및 캡처",
        );
        let upstream = self.t(
            "Open RenderDoc",
            "打开 RenderDoc",
            "RenderDoc を開く",
            "RenderDoc 열기",
        );
        let configure_ui = self.t(
            "Select qrenderdoc",
            "选择 qrenderdoc",
            "qrenderdoc を選択",
            "qrenderdoc 선택",
        );
        let configure_cmd = self.t(
            "Select renderdoccmd",
            "选择 renderdoccmd",
            "renderdoccmd を選択",
            "renderdoccmd 선택",
        );
        let configure_bridge = self.t(
            "Select replay bridge",
            "选择回放桥接程序",
            "リプレイブリッジを選択",
            "리플레이 브리지 선택",
        );
        let language = self.t("LANGUAGE", "语言", "言語", "언어");
        let integration = self.t(
            "UNITY / TUANJIE",
            "UNITY / 团结引擎",
            "UNITY / TUANJIE",
            "UNITY / TUANJIE",
        );
        let integration_text = self.t("Use the engine's RenderDoc integration or launch a standalone player. Detailed inspection currently opens in RenderDoc.", "使用引擎自带的 RenderDoc 集成，或启动独立播放器。详细调试目前在 RenderDoc 中进行。", "エンジンの RenderDoc 統合、またはスタンドアロンプレイヤーを使用してください。詳細な解析は現在 RenderDoc で開きます。", "엔진의 RenderDoc 통합을 사용하거나 독립 실행형 플레이어를 실행하세요. 상세 분석은 현재 RenderDoc에서 열립니다.");
        let status = self.status.clone();

        div()
            .size_full()
            .bg(rgb(0x101722))
            .text_color(rgb(0xe6edf6))
            .flex()
            .flex_col()
            .p_8()
            .gap_6()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x82aaff))
                    .child("RENDERDOCX  /  WORKSPACE"),
            )
            .child(
                div()
                    .text_3xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(title),
            )
            .child(div().text_color(rgb(0x9eacc0)).child(intro))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(button(
                        "open",
                        open,
                        true,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("RenderDoc", &["rdc", "cap"])
                                .pick_file()
                            {
                                this.open(path, cx);
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "launch",
                        launch,
                        false,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                let settings = this.settings.clone();
                                this.status = Some(
                                    this.t(
                                        "Starting capture target…",
                                        "正在启动捕获目标…",
                                        "キャプチャ対象を起動中…",
                                        "캡처 대상을 시작하는 중…",
                                    )
                                    .to_owned(),
                                );
                                cx.notify();
                                let job = cx.background_executor().spawn(async move {
                                    backend::launch_capture(&settings, &path)
                                });
                                cx.spawn(async move |this, cx| {
                                    let result = job.await;
                                    let _ = this.update(cx, |this, cx| this.result(result, cx));
                                })
                                .detach();
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "upstream",
                        upstream,
                        false,
                        |this, cx| {
                            let result = backend::open_upstream(&this.settings);
                            this.result(result, cx);
                        },
                        cx,
                    ))
                    .child(button(
                        "configure-bridge",
                        configure_bridge,
                        false,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                this.settings.replay_bridge = Some(path);
                                this.save();
                                cx.notify();
                            }
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .bg(rgb(0x192332))
                    .rounded_lg()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_sm().text_color(rgb(0x82aaff)).child(integration))
                    .child(div().text_color(rgb(0xc3cedd)).child(integration_text)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(button(
                        "configure-ui",
                        configure_ui,
                        false,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                this.settings.qrenderdoc = Some(path);
                                this.save();
                                cx.notify();
                            }
                        },
                        cx,
                    ))
                    .child(button(
                        "configure-cmd",
                        configure_cmd,
                        false,
                        |this, cx| {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                this.settings.renderdoccmd = Some(path);
                                this.save();
                                cx.notify();
                            }
                        },
                        cx,
                    )),
            )
            .child(div().text_sm().text_color(rgb(0x9eacc0)).child(language))
            .child(div().flex().flex_wrap().gap_2().children(
                Language::ALL.into_iter().enumerate().map(|(i, lang)| {
                    div()
                        .id(SharedString::from(format!("language-{i}")))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(rgb(if self.settings.language == lang {
                            0x4169e1
                        } else {
                            0x252f40
                        }))
                        .child(lang.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.language = lang;
                            this.save();
                            cx.notify();
                        }))
                }),
            ))
            .when_some(status, |s, status| {
                s.child(div().text_color(rgb(0xd4b77c)).child(status))
            })
            .into_any_element()
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
            let settings = Settings::load();
            let initial_capture = std::env::var_os("RENDERDOCX_OPEN_CAPTURE")
                .or_else(|| std::env::args_os().nth(1))
                .map(std::path::PathBuf::from);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    let entity = cx.new(|_| Workspace {
                        settings,
                        status: None,
                        capture: None,
                        session: None,
                        snapshot: None,
                        selected: None,
                        show_textures: false,
                        selected_texture: None,
                        open_revision: 0,
                        selection_revision: 0,
                    });
                    if let Some(path) = initial_capture {
                        entity.update(cx, |view, cx| view.open(path, cx));
                    }
                    entity
                },
            )
            .expect("Failed to open RenderDocX window");
            cx.activate(true);
        });
}

#[cfg(all(test, feature = "ui-tests"))]
mod ui_tests {
    use super::*;
    use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

    #[gpui_kit::test]
    fn capture_workspace_back_button_returns_to_home(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, view) = cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|_| Workspace {
                        settings: Settings::default(),
                        status: None,
                        capture: Some("capture.rdc".into()),
                        session: None,
                        snapshot: Some(Snapshot {
                            protocol: 1,
                            api: 3,
                            frame: 2,
                            degraded: false,
                            actions: Vec::new(),
                            resource_count: 0,
                            texture_count: 0,
                            buffer_count: 0,
                            textures: Vec::new(),
                        }),
                        selected: None,
                        show_textures: false,
                        selected_texture: None,
                        open_revision: 0,
                        selection_revision: 0,
                    })
                },
            )
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("capture-back", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(view.read(cx).snapshot.is_none());
            assert!(view.read(cx).capture.is_none());
        });
    }

    #[gpui_kit::test]
    fn clicking_an_action_replays_its_exact_event(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (session, events) = renderdocx::replay::test_support::recording_session();
        let (handle, view) = cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |_, cx| {
                    cx.new(|_| Workspace {
                        settings: Settings::default(),
                        status: None,
                        capture: Some("capture.rdc".into()),
                        session: Some(session),
                        snapshot: Some(Snapshot {
                            protocol: 1,
                            api: 0,
                            frame: 2,
                            degraded: false,
                            actions: vec![renderdocx::replay::Action {
                                event_id: 42,
                                action_id: 3,
                                depth: 0,
                                parent: -1,
                                flags: 2,
                                indices: 3,
                                instances: 1,
                                name: "Draw".into(),
                            }],
                            resource_count: 1,
                            texture_count: 1,
                            buffer_count: 0,
                            textures: vec![renderdocx::replay::Texture {
                                name: "Color".into(),
                                width: 16,
                                height: 16,
                                depth: 1,
                                dimension: 2,
                                mips: 1,
                                array_size: 1,
                                samples: 1,
                                bytes: "1024".into(),
                                format_type: 0,
                                component_type: 0,
                                component_count: 4,
                                component_byte_width: 1,
                            }],
                        }),
                        selected: None,
                        show_textures: false,
                        selected_texture: None,
                        open_revision: 0,
                        selection_revision: 0,
                    })
                },
            )
            .unwrap()
        });
        let handle = handle.downcast::<Root>().unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(0usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).selected, Some(0)));
        assert_eq!(*events.lock().unwrap(), vec![42]);
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("textures-tab", cx);
            window.click("texture-0", cx);
        })
        .unwrap();
        cx.update(|cx| {
            assert!(view.read(cx).show_textures);
            assert_eq!(view.read(cx).selected_texture, Some(0));
        });
        assert_eq!(*events.lock().unwrap(), vec![42]);
    }
}
