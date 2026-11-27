use std::{borrow::Cow, sync::OnceLock};

use gpui::{App, AssetSource, ParentElement, Rgba, SharedString, Styled, Svg, div, px, rgb, svg};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Workbench,
    Terminal,
    Studio,
    Ledger,
}

/// Every size and shape that distinguishes one appearance from another.
/// One static table per style keeps each lookup a single field read and keeps
/// the four designs comparable side by side.
pub struct Metrics {
    /// Top bar, status bar, request tab strip and sidebar heading heights.
    pub top: f32,
    pub status: f32,
    pub tab_strip: f32,
    pub sidebar_heading: f32,
    /// A single request tab inside the strip.
    pub tab: f32,
    pub tab_radius: f32,
    pub tab_gap: f32,
    /// Sidebar tree rows.
    pub row: f32,
    pub row_radius: f32,
    /// Address bar controls and the padding around them.
    pub command: f32,
    pub command_pad: f32,
    pub radius: f32,
    /// Request and response pane headings and their section tabs.
    pub pane_head: f32,
    pub section_tabs: f32,
    pub heading: f32,
    pub title: f32,
    pub sidebar_title: f32,
    /// Key-value table rows.
    pub kv_row: f32,
    /// Corner radius of small chips such as counts and statuses.
    pub chip_radius: f32,
    /// Body text size and the font used for chrome labels.
    pub text: f32,
    pub heading_font: Face,
    pub body_font: Face,
    /// Inset around the request and response panes; zero keeps them flush.
    pub pane_inset: f32,
    pub pane_radius: f32,
    /// Ledger-style double rules under the top bar and the address bar.
    pub double_rule: bool,
    /// Divider lines between tabs and under sidebar rows.
    pub divided_tabs: bool,
    pub ruled_rows: bool,
    /// Alternating backgrounds on key-value rows.
    pub striped_rows: bool,
    /// Left gutter rule beside the response body.
    pub gutter: f32,
}

const WORKBENCH: Metrics = Metrics {
    top: 50.,
    status: 26.,
    tab_strip: 40.,
    sidebar_heading: 43.,
    tab: 33.,
    tab_radius: 6.,
    tab_gap: 3.,
    row: 27.,
    row_radius: 5.,
    command: 40.,
    command_pad: 12.,
    radius: 8.,
    pane_head: 39.,
    section_tabs: 40.,
    heading: 14.,
    title: 16.,
    sidebar_title: 13.,
    kv_row: 32.,
    chip_radius: 6.,
    text: 13.,
    heading_font: Face::Sans,
    body_font: Face::Sans,
    pane_inset: 0.,
    pane_radius: 0.,
    double_rule: false,
    divided_tabs: false,
    ruled_rows: false,
    striped_rows: false,
    gutter: 0.,
};

const TERMINAL: Metrics = Metrics {
    top: 40.,
    status: 22.,
    tab_strip: 32.,
    sidebar_heading: 35.,
    tab: 26.,
    tab_radius: 2.,
    tab_gap: 2.,
    row: 23.,
    row_radius: 2.,
    command: 32.,
    command_pad: 8.,
    radius: 2.,
    pane_head: 32.,
    section_tabs: 32.,
    heading: 12.,
    title: 13.,
    sidebar_title: 12.,
    kv_row: 26.,
    chip_radius: 2.,
    text: 12.,
    heading_font: Face::Mono,
    body_font: Face::Mono,
    pane_inset: 0.,
    pane_radius: 0.,
    double_rule: false,
    divided_tabs: false,
    ruled_rows: false,
    striped_rows: true,
    gutter: 0.,
};

const STUDIO: Metrics = Metrics {
    top: 56.,
    status: 26.,
    tab_strip: 44.,
    sidebar_heading: 48.,
    tab: 30.,
    tab_radius: 999.,
    tab_gap: 6.,
    row: 30.,
    row_radius: 8.,
    command: 42.,
    command_pad: 12.,
    radius: 999.,
    pane_head: 44.,
    section_tabs: 42.,
    heading: 16.,
    title: 19.,
    sidebar_title: 15.,
    kv_row: 34.,
    chip_radius: 999.,
    text: 13.,
    heading_font: Face::Sans,
    body_font: Face::Sans,
    pane_inset: 8.,
    pane_radius: 14.,
    double_rule: false,
    divided_tabs: false,
    ruled_rows: false,
    striped_rows: false,
    gutter: 0.,
};

const LEDGER: Metrics = Metrics {
    top: 52.,
    status: 26.,
    tab_strip: 42.,
    sidebar_heading: 46.,
    tab: 42.,
    tab_radius: 0.,
    tab_gap: 0.,
    row: 28.,
    row_radius: 0.,
    command: 40.,
    command_pad: 12.,
    radius: 0.,
    pane_head: 42.,
    section_tabs: 38.,
    heading: 20.,
    title: 22.,
    sidebar_title: 18.,
    kv_row: 32.,
    chip_radius: 0.,
    text: 13.,
    heading_font: Face::Serif,
    body_font: Face::Sans,
    pane_inset: 0.,
    pane_radius: 0.,
    double_rule: true,
    divided_tabs: true,
    ruled_rows: true,
    striped_rows: false,
    gutter: 3.,
};

impl Appearance {
    pub fn read(settings: &Value) -> Self {
        match settings["general"]["appearance"].as_str() {
            Some("terminal") => Self::Terminal,
            Some("studio") => Self::Studio,
            Some("ledger") => Self::Ledger,
            _ => Self::Workbench,
        }
    }

    pub const fn metrics(self) -> &'static Metrics {
        match self {
            Self::Workbench => &WORKBENCH,
            Self::Terminal => &TERMINAL,
            Self::Studio => &STUDIO,
            Self::Ledger => &LEDGER,
        }
    }
}

impl std::ops::Deref for Appearance {
    type Target = Metrics;
    fn deref(&self) -> &Metrics {
        self.metrics()
    }
}

/// Height of the double rule Ledger draws under its top and address bars.
pub const DOUBLE_RULE: f32 = 3.;

/// A role in the type system, resolved to an installed family at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Sans,
    Mono,
    Serif,
}

impl Face {
    pub fn family(self) -> &'static str {
        let [sans, mono, serif] = *FAMILIES.get_or_init(|| fallback_families(&[]));
        match self {
            Self::Sans => sans,
            Self::Mono => mono,
            Self::Serif => serif,
        }
    }
}

pub fn mono() -> &'static str {
    Face::Mono.family()
}

static FAMILIES: OnceLock<[&'static str; 3]> = OnceLock::new();

#[cfg(target_os = "macos")]
const CANDIDATES: [&[&str]; 3] = [
    &[".AppleSystemUIFont"],
    &["SF Mono", "Menlo", "Monaco"],
    &["New York", "Charter", "Georgia", "Times New Roman"],
];
#[cfg(target_os = "windows")]
const CANDIDATES: [&[&str]; 3] = [
    &["Segoe UI"],
    &["Cascadia Mono", "Consolas", "Courier New"],
    &["Georgia", "Cambria", "Times New Roman"],
];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const CANDIDATES: [&[&str]; 3] = [
    &[
        "Inter",
        "Noto Sans",
        "Cantarell",
        "Ubuntu",
        "DejaVu Sans",
        "Liberation Sans",
    ],
    &[
        "JetBrains Mono",
        "Fira Code",
        "Noto Sans Mono",
        "DejaVu Sans Mono",
        "Ubuntu Mono",
        "Liberation Mono",
    ],
    &[
        "Noto Serif",
        "Source Serif 4",
        "Charter",
        "DejaVu Serif",
        "Liberation Serif",
        "Georgia",
    ],
];

/// Picks the first preferred family of each role that is installed. The
/// generic CSS names are a last resort: the Linux text system does not map
/// them, so `serif` and `monospace` otherwise render in the default sans.
fn fallback_families(installed: &[String]) -> [&'static str; 3] {
    const GENERIC: [&str; 3] = ["sans-serif", "monospace", "serif"];
    std::array::from_fn(|role| {
        CANDIDATES[role]
            .iter()
            .copied()
            .find(|name| installed.iter().any(|found| found == name))
            .or(if installed.is_empty() {
                CANDIDATES[role].first().copied()
            } else {
                None
            })
            .unwrap_or(GENERIC[role])
    })
}

/// Resolves the type families once, before the first window renders.
pub fn load_fonts(cx: &App) {
    let installed = cx.text_system().all_font_names();
    let _ = FAMILIES.set(fallback_families(&installed));
}

#[derive(Clone)]
pub struct Palette {
    pub appearance: Appearance,
    pub dark: bool,
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
    pub primary: Rgba,
    pub primary_text: Rgba,
    pub primary_hover: Rgba,
    pub primary_active: Rgba,
    pub selected: Rgba,
    pub cyan: Rgba,
    pub ok: Rgba,
    pub warn: Rgba,
    pub error: Rgba,
    pub info: Rgba,
    pub violet: Rgba,
    /// JSON syntax: object keys and string values.
    pub key: Rgba,
    pub string: Rgba,
}

fn accent_tones(name: &str) -> (u32, u32) {
    match name {
        "ocean" => (0x38bdf8, 0x0369a1),
        "mint" => (0x34d399, 0x047857),
        "violet" => (0xa78bfa, 0x6d28d9),
        "rose" => (0xfb7185, 0xbe123c),
        "amber" => (0xfbbf24, 0xb45309),
        "teal" => (0x2dd4bf, 0x0f766e),
        "magenta" => (0xe879f9, 0xa21caf),
        _ => (0xff8964, 0xae3218),
    }
}

pub fn primary_fill(name: &str, dark: bool) -> Rgba {
    let (bright, deep) = accent_tones(name);
    if dark && matches!(name, "mint" | "amber" | "teal") {
        rgb(bright)
    } else if dark {
        tint(rgb(deep), rgb(bright), 0.08)
    } else {
        rgb(deep)
    }
}

fn tint(base: Rgba, color: Rgba, amount: f32) -> Rgba {
    base.blend(Rgba { a: amount, ..color })
}

fn luminance(color: Rgba) -> f32 {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn ink_on(fill: Rgba) -> Rgba {
    let dark = tint(rgb(0x080808), fill, 0.18);
    let light = tint(rgb(0xffffff), fill, 0.035);
    let surface = luminance(fill) + 0.05;
    if surface / (luminance(dark) + 0.05) > (luminance(light) + 0.05) / surface {
        dark
    } else {
        light
    }
}

impl Palette {
    pub fn new(settings: &Value, dark: bool) -> Self {
        let choose = |night, day| rgb(if dark { night } else { day });
        let name = settings["general"]["accent"].as_str().unwrap_or("ember");
        let (bright, deep) = accent_tones(name);
        let accent = choose(bright, deep);
        let primary = primary_fill(name, dark);
        let primary_text = ink_on(primary);
        let edge = if luminance(primary_text) > luminance(primary) {
            rgb(0)
        } else {
            rgb(0xffffff)
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
            dark,
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
            accent,
            accent_text: ink_on(accent),
            primary,
            primary_text,
            primary_hover: tint(primary, edge, 0.10),
            primary_active: tint(primary, edge, 0.19),
            selected: tint(side, accent, if dark { 0.15 } else { 0.10 }),
            cyan: choose(0x35c9dd, 0x096b7b),
            ok: choose(0x46d68c, 0x176b42),
            warn: choose(0xf2b544, 0x825400),
            error: choose(0xff5d73, 0xba2942),
            info: choose(0x58a6ff, 0x2265a9),
            violet: choose(0xb78cff, 0x7150a5),
            key: choose(0x9bd5df, 0x096578),
            string: choose(0xa6dcb6, 0x236d39),
        }
    }

    /// Closes a horizontal bar with the appearance's bottom rule: a double rule
    /// for Ledger and a single hairline elsewhere. The double rule sits inside
    /// the bar so every appearance keeps the same outer height.
    pub fn underline<E: Styled + ParentElement>(&self, bar: E) -> E {
        if self.appearance.double_rule {
            bar.relative().child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(DOUBLE_RULE))
                    .border_t_1()
                    .border_b_1()
                    .border_color(self.line),
            )
        } else {
            bar.border_b_1().border_color(self.line)
        }
    }

    /// Background of the even key-value rows when the appearance stripes them.
    pub fn stripe(&self, index: usize) -> Option<Rgba> {
        (self.appearance.striped_rows && index % 2 == 1).then_some(self.bg)
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
        let name = path
            .strip_prefix("postlite/")
            .and_then(|p| p.strip_suffix(".svg"))
            .or_else(|| match path {
                "icons/close.svg" => Some("x"),
                "icons/check.svg" => Some("check"),
                "icons/search.svg" => Some("search"),
                "icons/eye.svg" => Some("eye"),
                "icons/eye-off.svg" => Some("eye-off"),
                _ => None,
            });
        let Some(name) = name else {
            return Ok(None);
        };
        let markup = if name == "brand" {
            Some(
                "viewBox=\"0 0 24 24\"><path fill=\"none\" stroke=\"white\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"M5.4 12h3l1.7-4 2.6 8 1.7-4h4.2\"/>",
            )
        } else if name == "send" {
            Some(
                r#"viewBox="0 0 24 24"><path fill="none" stroke="white" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" d="M21 3 3 10l7 3 3 7 8-17Zm0 0L10 13"/>"#,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_roles_prefer_installed_families_and_never_return_empty() {
        let installed = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let [_, mono, serif] = fallback_families(&installed(&[CANDIDATES[1][2], CANDIDATES[2][1]]));
        assert_eq!((mono, serif), (CANDIDATES[1][2], CANDIDATES[2][1]));
        assert_eq!(
            fallback_families(&installed(&["Nothing Matching"])),
            ["sans-serif", "monospace", "serif"]
        );
    }

    #[test]
    fn every_appearance_keeps_the_shared_layout_budget() {
        for appearance in [
            Appearance::Workbench,
            Appearance::Terminal,
            Appearance::Studio,
            Appearance::Ledger,
        ] {
            assert!(appearance.tab <= appearance.tab_strip, "{appearance:?}");
            assert!(
                appearance.command < appearance.tab_strip + 12.,
                "{appearance:?}"
            );
            assert!(
                appearance.gutter == 0. || appearance.pane_inset == 0.,
                "{appearance:?}"
            );
        }
    }

    #[test]
    fn primary_labels_keep_contrast_across_accents_and_interaction_states() {
        for name in [
            "ember", "ocean", "mint", "violet", "rose", "amber", "teal", "magenta",
        ] {
            for dark in [false, true] {
                let palette = Palette::new(&serde_json::json!({"general":{"accent":name}}), dark);
                let ink = luminance(palette.primary_text) + 0.05;
                for fill in [
                    palette.primary,
                    palette.primary_hover,
                    palette.primary_active,
                ] {
                    let surface = luminance(fill) + 0.05;
                    let ratio = ink.max(surface) / ink.min(surface);
                    assert!(ratio >= 4.5, "{name} dark={dark}: {ratio}");
                }
            }
        }
    }
}
