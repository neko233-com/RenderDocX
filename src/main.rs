mod backend;
mod i18n;

use backend::Settings;
use gpui::{
    App, Application, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use i18n::Language;

struct Workspace {
    settings: Settings,
    status: Option<String>,
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
}

fn button(
    id: &'static str,
    title: &'static str,
    accent: bool,
    callback: impl Fn(&mut Workspace, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(id)
        .px_4()
        .py_3()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(if accent { 0x4169e1 } else { 0x252f40 }))
        .hover(|s| s.opacity(0.8))
        .text_color(rgb(0xffffff))
        .child(title)
        .on_click(cx.listener(move |this, _, _, cx| callback(this, cx)))
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                                let result = backend::open_capture(&this.settings, &path);
                                this.result(result, cx);
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
                                let result = backend::launch_capture(&this.settings, &path);
                                this.result(result, cx);
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
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1040.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Workspace {
                    settings: Settings::load(),
                    status: None,
                })
            },
        )
        .expect("Failed to open RenderDocX window");
        cx.activate(true);
    });
}
