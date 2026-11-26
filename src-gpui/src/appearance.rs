use std::borrow::Cow;

use gpui::{AssetSource, Rgba, SharedString, Styled, Svg, px, rgb, svg};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Workbench,
    Terminal,
    Studio,
    Ledger,
}

impl Appearance {
    pub fn read(settings: &Value) -> Self {
        match settings["general"]["appearance"].as_str() {
            Some("terminal") => Self::Terminal,
            Some("studio") => Self::Studio,
            Some("ledger") => Self::Ledger,
            _ => Self::Workbench,
        }
    }

    pub fn top(self) -> f32 {
        match self {
            Self::Terminal => 41.,
            Self::Studio => 58.,
            Self::Ledger => 52.,
            _ => 50.,
        }
    }
    pub fn status(self) -> f32 {
        if self == Self::Terminal { 22. } else { 26. }
    }
    pub fn tab(self) -> f32 {
        match self {
            Self::Terminal => 33.,
            Self::Studio => 45.,
            Self::Ledger => 42.,
            _ => 40.,
        }
    }
    pub fn sidebar_heading(self) -> f32 {
        match self {
            Self::Terminal => 35.,
            Self::Studio => 48.,
            Self::Ledger => 46.,
            _ => 43.,
        }
    }
    pub fn row(self) -> f32 {
        match self {
            Self::Terminal => 23.,
            Self::Studio => 29.,
            _ => 27.,
        }
    }
    pub fn command(self) -> f32 {
        match self {
            Self::Terminal => 32.,
            Self::Studio => 44.,
            Self::Ledger => 42.,
            _ => 40.,
        }
    }
    pub fn radius(self) -> f32 {
        match self {
            Self::Terminal => 2.,
            Self::Studio => 999.,
            Self::Ledger => 0.,
            _ => 8.,
        }
    }
    pub fn heading(self) -> f32 {
        match self {
            Self::Terminal => 12.,
            Self::Studio => 16.,
            Self::Ledger => 22.,
            _ => 14.,
        }
    }
    pub fn title(self) -> f32 {
        match self {
            Self::Terminal => 12.,
            Self::Studio => 19.,
            Self::Ledger => 22.,
            _ => 16.,
        }
    }
    pub fn heading_font(self) -> &'static str {
        match self {
            Self::Terminal => MONO,
            Self::Ledger => SERIF,
            _ => SANS,
        }
    }
}

#[cfg(target_os = "windows")]
pub const SANS: &str = "Segoe UI";
#[cfg(target_os = "macos")]
pub const SANS: &str = ".AppleSystemUIFont";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const SANS: &str = "sans-serif";
pub const MONO: &str = "monospace";
pub const SERIF: &str = "serif";

pub struct Palette {
    pub appearance: Appearance,
    pub bg: Rgba,
    pub panel: Rgba,
    pub raised: Rgba,
    pub hover: Rgba,
    pub side: Rgba,
    pub input: Rgba,
    pub line: Rgba,
    pub soft_line: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub faint: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    pub cyan: Rgba,
    pub ok: Rgba,
    pub warn: Rgba,
    pub error: Rgba,
    pub info: Rgba,
    pub violet: Rgba,
}

impl Palette {
    pub fn new(settings: &Value, dark: bool) -> Self {
        let choose = |night, day| rgb(if dark { night } else { day });
        let (accent_dark, accent_light) = match settings["general"]["accent"].as_str() {
            Some("ocean") => (0x38bdf8, 0x0369a1),
            Some("mint") => (0x34d399, 0x047857),
            Some("violet") => (0xa78bfa, 0x6d28d9),
            Some("rose") => (0xfb7185, 0xbe123c),
            Some("amber") => (0xfbbf24, 0xb45309),
            Some("teal") => (0x2dd4bf, 0x0f766e),
            Some("magenta") => (0xe879f9, 0xa21caf),
            _ => (0xff8964, 0xae3218),
        };
        let appearance = Appearance::read(settings);
        let surfaces = match (appearance, dark) {
            (Appearance::Terminal, true) => [
                0x101517, 0x161e20, 0x1d292b, 0x283638, 0x121a1c, 0x0c1213, 0x3b5050, 0x293939,
                0xe2eee9, 0xb4c8c0, 0x9bafa7,
            ],
            (Appearance::Terminal, false) => [
                0xf4f7f5, 0xfbfdfb, 0xe3ebe6, 0xd6e2da, 0xe8efea, 0xfcfefc, 0xb5c4ba, 0xd5e0d7,
                0x182923, 0x405750, 0x4f655d,
            ],
            (Appearance::Studio, true) => [
                0x191a26, 0x252637, 0x2e3045, 0x3a3d55, 0x202132, 0x1d1e2e, 0x4b4e68, 0x3b3d54,
                0xf5f3ff, 0xd3cfe8, 0xb7b3ce,
            ],
            (Appearance::Studio, false) => [
                0xf1effa, 0xfdfbff, 0xede9f8, 0xe3dcf2, 0xf2eef9, 0xfdfbff, 0xc9c0d9, 0xe2daed,
                0x272438, 0x4d4664, 0x5e5674,
            ],
            (Appearance::Ledger, true) => [
                0x20211d, 0x292b24, 0x303228, 0x393c30, 0x25271e, 0x22241c, 0x545745, 0x3a3e31,
                0xf3efde, 0xd1cdb8, 0xb6b099,
            ],
            (Appearance::Ledger, false) => [
                0xf6f3e8, 0xfdfaf0, 0xefebdb, 0xe5e0ce, 0xece8d8, 0xfdfbf2, 0xc4bea8, 0xe4decb,
                0x2d2d24, 0x525242, 0x5f5e4d,
            ],
            (Appearance::Workbench, true) => [
                0x1b1815, 0x251f1b, 0x2e2721, 0x3a3029, 0x211c18, 0x191613, 0x483c33, 0x362c25,
                0xfff3e5, 0xd1c1b2, 0xb2a092,
            ],
            (Appearance::Workbench, false) => [
                0xf8f4eb, 0xfffaf1, 0xf0e9de, 0xe8ddcf, 0xf1ebe0, 0xfffdf7, 0xcfc1b2, 0xe3d8c9,
                0x261c16, 0x56483e, 0x6a594d,
            ],
        };
        let [
            bg,
            panel,
            raised,
            hover,
            side,
            input,
            line,
            soft_line,
            text,
            dim,
            faint,
        ] = surfaces.map(rgb);
        Self {
            appearance,
            bg,
            panel,
            raised,
            hover,
            side,
            input,
            line,
            soft_line,
            text,
            dim,
            faint,
            accent: choose(accent_dark, accent_light),
            accent_text: choose(0x200f0b, 0xfffaf1),
            cyan: choose(0x35c9dd, 0x096b7b),
            ok: choose(0x46d68c, 0x176b42),
            warn: choose(0xf2b544, 0x825400),
            error: choose(0xff5d73, 0xba2942),
            info: choose(0x58a6ff, 0x2265a9),
            violet: choose(0xb78cff, 0x7150a5),
        }
    }

    pub fn method(&self, method: &str) -> Rgba {
        match method {
            "GET" => self.ok,
            "POST" => self.warn,
            "PUT" => self.info,
            "PATCH" => self.violet,
            "DELETE" => self.error,
            _ => self.cyan,
        }
    }
}

pub fn icon(name: &'static str, color: Rgba, size: f32) -> Svg {
    svg()
        .path(format!("postlite/{name}.svg"))
        .size(px(size))
        .text_color(color)
}

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        const SOURCE: &str = include_str!("../../ui/index.html");
        let Some(name) = path
            .strip_prefix("postlite/")
            .and_then(|p| p.strip_suffix(".svg"))
        else {
            return Ok(None);
        };
        let markup = if name == "brand" {
            Some(
                "viewBox=\"0 0 24 24\"><path fill=\"none\" stroke=\"white\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"M5.4 12h3l1.7-4 2.6 8 1.7-4h4.2\"/>",
            )
        } else if name == "chevron-down" {
            Some(
                "viewBox=\"0 0 16 16\"><path fill=\"none\" stroke=\"white\" stroke-width=\"1.3\" stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"m3.5 5.5 4.5 5.5 4.5-5.5\"/>",
            )
        } else {
            SOURCE
                .split_once(&format!("<symbol id=\"i-{name}\" "))
                .and_then(|(_, rest)| rest.split_once("</symbol>"))
                .map(|(markup, _)| markup)
        };
        Ok(markup.map(|markup| {
            Cow::Owned(
                format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" {} </svg>",
                    markup.replace("currentColor", "white"),
                )
                .into_bytes(),
            )
        }))
    }

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
