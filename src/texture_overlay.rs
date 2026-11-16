//! Texture viewer overlays, in the order of the matched RenderDoc API.
use crate::i18n::Language;
use serde::{Deserialize, Serialize};

#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureOverlay {
    #[default]
    None = 0,
    Drawcall = 1,
    Wireframe = 2,
    Depth = 3,
    Stencil = 4,
    BackfaceCull = 5,
    ViewportScissor = 6,
    Nan = 7,
    Clipping = 8,
    ClearBeforePass = 9,
    ClearBeforeDraw = 10,
    QuadOverdrawPass = 11,
    QuadOverdrawDraw = 12,
    TriangleSizePass = 13,
    TriangleSizeDraw = 14,
}
impl TextureOverlay {
    pub const ALL: [Self; 15] = [
        Self::None,
        Self::Drawcall,
        Self::Wireframe,
        Self::Depth,
        Self::Stencil,
        Self::BackfaceCull,
        Self::ViewportScissor,
        Self::Nan,
        Self::Clipping,
        Self::ClearBeforePass,
        Self::ClearBeforeDraw,
        Self::QuadOverdrawPass,
        Self::QuadOverdrawDraw,
        Self::TriangleSizePass,
        Self::TriangleSizeDraw,
    ];
    pub fn uses_overlay_pixel(self) -> bool {
        matches!(
            self,
            Self::QuadOverdrawPass
                | Self::QuadOverdrawDraw
                | Self::TriangleSizePass
                | Self::TriangleSizeDraw
        )
    }
    pub fn next(self) -> Self {
        Self::ALL[((self as usize) + 1) % Self::ALL.len()]
    }
    pub fn label(self, language: Language) -> &'static str {
        match self {
            Self::None => language.text("None", "无", "なし", "없음"),
            Self::Drawcall => language.text("Drawcall", "绘制覆盖", "描画範囲", "그리기 영역"),
            Self::Wireframe => {
                language.text("Wireframe", "线框", "ワイヤーフレーム", "와이어프레임")
            }
            Self::Depth => language.text("Depth test", "深度测试", "深度テスト", "깊이 테스트"),
            Self::Stencil => language.text(
                "Stencil test",
                "模板测试",
                "ステンシルテスト",
                "스텐실 테스트",
            ),
            Self::BackfaceCull => {
                language.text("Backface culling", "背面剔除", "背面カリング", "뒷면 컬링")
            }
            Self::ViewportScissor => language.text(
                "Viewport and scissor",
                "视口与裁剪",
                "ビューポートとシザー",
                "뷰포트와 시저",
            ),
            Self::Nan => language.text(
                "NaN and infinity",
                "NaN 与无穷值",
                "NaN と無限大",
                "NaN 및 무한대",
            ),
            Self::Clipping => language.text("Clipping", "范围裁剪", "クリッピング", "클리핑"),
            Self::ClearBeforePass => language.text(
                "Clear before pass",
                "通道前清除",
                "パス前にクリア",
                "패스 전 지우기",
            ),
            Self::ClearBeforeDraw => language.text(
                "Clear before draw",
                "绘制前清除",
                "描画前にクリア",
                "그리기 전 지우기",
            ),
            Self::QuadOverdrawPass => language.text(
                "Quad overdraw: pass",
                "四像素过度绘制：通道",
                "クアッドオーバードロー：パス",
                "쿼드 오버드로: 패스",
            ),
            Self::QuadOverdrawDraw => language.text(
                "Quad overdraw: draw",
                "四像素过度绘制：绘制",
                "クアッドオーバードロー：描画",
                "쿼드 오버드로: 그리기",
            ),
            Self::TriangleSizePass => language.text(
                "Triangle size: pass",
                "三角形尺寸：通道",
                "三角形サイズ：パス",
                "삼각형 크기: 패스",
            ),
            Self::TriangleSizeDraw => language.text(
                "Triangle size: draw",
                "三角形尺寸：绘制",
                "三角形サイズ：描画",
                "삼각형 크기: 그리기",
            ),
        }
    }
}
