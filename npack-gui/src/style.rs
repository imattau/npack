//! Look and feel: type scale, spacing, accent colour and font fallbacks.
//!
//! Evermore Graphite: dark-first, low-chroma graphite neutrals, one
//! cool-blue accent, amber reserved for attention, red for destructive
//! actions (PROJECT_IDEA.md's "Visual Design System — Evermore
//! Graphite" design note, `github.com/imattau/evermore`). egui still
//! follows the system light/dark setting (`ThemePreference::System`);
//! both themes get their own palette here rather than one set of tweaks
//! shared across both, since light and dark need different absolute
//! colour gaps to read as the same contrast (see `LIGHT`'s own note).
//! Fonts are unchanged from before: nothing is bundled, so this only
//! touches colour, spacing, radius and corner treatment, never which
//! font paints a glyph.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle, Theme};

/// One theme's worth of Graphite tokens.
struct Palette {
    /// Application canvas / `extreme_bg_color`.
    canvas: Color32,
    /// Window and panel chrome.
    chrome: Color32,
    /// Cards, list rows, soft-fill sections (`faint_bg_color`).
    card: Color32,
    text: Color32,
    text_muted: Color32,
}

/// `ink-950`/`ink-900`/`ink-850`, primary/muted text -- dark-first, so
/// this is the one `ThemePreference::Dark` would pick without the
/// system asking for light.
const DARK: Palette = Palette {
    canvas: Color32::from_rgb(14, 16, 20),
    chrome: Color32::from_rgb(20, 23, 29),
    card: Color32::from_rgb(26, 30, 37),
    text: Color32::from_rgb(230, 233, 239),
    text_muted: Color32::from_rgb(167, 176, 191),
};

/// A small absolute delta from white reads as flat at this luminance
/// (perceived contrast scales with luminance, not raw RGB difference)
/// even though the same delta worked for `DARK`, so this canvas/card
/// gap is deliberately much wider in absolute terms than `DARK`'s is.
const LIGHT: Palette = Palette {
    canvas: Color32::from_rgb(255, 255, 255),
    chrome: Color32::from_rgb(255, 255, 255),
    card: Color32::from_rgb(223, 226, 231),
    text: Color32::from_rgb(20, 23, 29),
    text_muted: Color32::from_rgb(90, 98, 112),
};

/// Selection, links, focus base, press/active -- "blue means
/// interactive," never a large background wash.
pub const ACCENT: Color32 = Color32::from_rgb(110, 168, 254);
/// Hover and focus ring.
const ACCENT_STRONG: Color32 = Color32::from_rgb(139, 183, 255);
/// Text on an accent fill. `ACCENT` is too light for white text to
/// clear WCAG AA (confirmed by this file's own contrast test) -- this
/// near-black navy is the token the design note defines for exactly
/// that purpose, at 7.8:1 on `ACCENT`.
const ACCENT_INK: Color32 = Color32::from_rgb(11, 16, 32);
/// Attention, not error (pending approvals, warnings).
const WARNING: Color32 = Color32::from_rgb(229, 181, 103);
/// Destructive, critical, errors.
const DANGER: Color32 = Color32::from_rgb(224, 108, 117);

/// Biggest font file read as a fallback. CJK collections are large, but a
/// runaway or hostile file in a font directory should not be loaded whole.
const MAX_FONT_BYTES: u64 = 40 * 1024 * 1024;

/// Fonts that cover more than Latin, in priority order. Looked up in the usual
/// Linux font directories; whichever exist are added after egui's own fonts.
const FALLBACK_FONT_FILES: &[&str] = &[
    "NotoSans-Regular.ttf",
    "NotoSansCJK-Regular.ttc",
    "NotoSansArabic-Regular.ttf",
    "NotoSansDevanagari-Regular.ttf",
    "NotoSansSymbols2-Regular.ttf",
    "DejaVuSans.ttf",
];

const FONT_DIRECTORIES: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/usr/share/fonts/truetype",
    "/usr/share/fonts/opentype",
];

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts_with_fallbacks(&find_fallback_fonts(
        FONT_DIRECTORIES.iter().map(Path::new),
    )));
    for (theme, palette) in [(Theme::Dark, &DARK), (Theme::Light, &LIGHT)] {
        let mut visuals = match theme {
            Theme::Dark => egui::Visuals::dark(),
            Theme::Light => egui::Visuals::light(),
        };
        apply_palette(&mut visuals, palette);
        ctx.set_visuals_of(theme, visuals);
        let mut style = (*ctx.style_of(theme)).clone();
        apply_style(&mut style);
        ctx.set_style_of(theme, style);
    }
    // Both DARK and LIGHT are Evermore's own palettes (not egui's stock
    // Visuals::dark()/light() with a tweak or two) -- "dark-first" here
    // means dark is the primary design target, not that this GUI should
    // override the person's own light/dark preference. Deliberately not
    // touching ctx.set_theme: it already defaults to
    // ThemePreference::System, so whichever theme the OS is in just
    // gets Evermore's matching palette instead of egui's defaults.
}

fn apply_palette(visuals: &mut egui::Visuals, palette: &Palette) {
    visuals.panel_fill = palette.chrome;
    visuals.window_fill = palette.chrome;
    visuals.extreme_bg_color = palette.canvas;
    visuals.faint_bg_color = palette.card;
    visuals.weak_text_color = Some(palette.text_muted);
    visuals.override_text_color = Some(palette.text);
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = DANGER;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);

    let text_stroke = egui::Stroke::new(1.0, palette.text);
    visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = text_stroke;
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = text_stroke;
    // "Blue means interactive": hover/press feedback and the scrollbar
    // handle share these tokens (egui has no way to tint the scrollbar
    // alone without it).
    visuals.widgets.hovered.bg_fill = ACCENT_STRONG.gamma_multiply(0.55);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = text_stroke;
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.fg_stroke = text_stroke;

    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.open.corner_radius = egui::CornerRadius::same(12);
}

/// Spacing, padding and type scale -- unchanged from npack-gui's own
/// sizes (not Evermore's panel's smaller, denser scale: that would drop
/// body text below this file's own readability floor, a deliberate
/// upstream choice this change doesn't touch). Only colour, spacing
/// rhythm and corner radius come from the design note here.
fn apply_style(style: &mut egui::Style) {
    use TextStyle::*;
    style.text_styles = [
        (Heading, FontId::new(22.0, FontFamily::Proportional)),
        (Body, FontId::new(15.0, FontFamily::Proportional)),
        (Button, FontId::new(15.0, FontFamily::Proportional)),
        (Small, FontId::new(12.5, FontFamily::Proportional)),
        (Monospace, FontId::new(14.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 32.0;
    style.spacing.window_margin = egui::Margin::same(16);
}

/// The main action on a screen: filled with the accent colour, text in
/// `ACCENT_INK` (not white -- see that constant's own doc comment).
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).color(ACCENT_INK)).fill(ACCENT)
}

/// Every font file under `directories` whose name is in `FALLBACK_FONT_FILES`
/// (searched two levels down, which covers the usual `<dir>/<family>/<file>`
/// layout), in priority order, each at most once.
pub fn find_fallback_fonts<'a>(directories: impl Iterator<Item = &'a Path>) -> Vec<PathBuf> {
    let mut found: Vec<Option<PathBuf>> = vec![None; FALLBACK_FONT_FILES.len()];
    for directory in directories {
        let mut pending = vec![(directory.to_path_buf(), 0)];
        while let Some((current, depth)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                // Symlinked directories are skipped, so a loop cannot trap us.
                if kind.is_dir() && depth < 2 {
                    pending.push((path, depth + 1));
                } else if kind.is_file() || kind.is_symlink() {
                    let name = entry.file_name();
                    if let Some(slot) = FALLBACK_FONT_FILES
                        .iter()
                        .position(|wanted| name.to_str() == Some(wanted))
                        && found[slot].is_none()
                    {
                        found[slot] = Some(path);
                    }
                }
            }
        }
    }
    found.into_iter().flatten().collect()
}

/// egui's default fonts, with each readable file in `files` added as a
/// fallback for both families. Unreadable, oversized or empty files are
/// skipped.
pub fn fonts_with_fallbacks(files: &[PathBuf]) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for (index, path) in files.iter().enumerate() {
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if metadata.len() == 0 || metadata.len() > MAX_FONT_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = format!("fallback-{index}");
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
    }
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_fonts_are_found_in_priority_order_and_only_once() {
        let root = tempfile::tempdir().unwrap();
        let noto = root.path().join("truetype/noto");
        let dejavu = root.path().join("truetype/dejavu");
        std::fs::create_dir_all(&noto).unwrap();
        std::fs::create_dir_all(&dejavu).unwrap();
        std::fs::write(dejavu.join("DejaVuSans.ttf"), b"x").unwrap();
        std::fs::write(noto.join("NotoSans-Regular.ttf"), b"x").unwrap();
        std::fs::write(noto.join("unrelated.ttf"), b"x").unwrap();
        // A second copy elsewhere must not be added again.
        std::fs::write(root.path().join("NotoSans-Regular.ttf"), b"x").unwrap();
        let found = find_fallback_fonts([root.path()].into_iter());
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["NotoSans-Regular.ttf", "DejaVuSans.ttf"]);
    }

    #[test]
    fn missing_directories_and_bad_files_are_ignored() {
        assert!(find_fallback_fonts([Path::new("/no/such/dir")].into_iter()).is_empty());
        let root = tempfile::tempdir().unwrap();
        let empty = root.path().join("empty.ttf");
        std::fs::write(&empty, b"").unwrap();
        let fonts = fonts_with_fallbacks(&[empty, root.path().join("missing.ttf")]);
        assert_eq!(
            fonts.font_data.len(),
            FontDefinitions::default().font_data.len(),
            "nothing is added for an empty or missing file"
        );
    }

    #[test]
    fn a_readable_font_is_added_to_both_families_after_the_defaults() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("f.ttf");
        std::fs::write(&file, b"not really a font").unwrap();
        let defaults = FontDefinitions::default();
        let fonts = fonts_with_fallbacks(&[file]);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let list = &fonts.families[&family];
            assert_eq!(list.len(), defaults.families[&family].len() + 1);
            assert_eq!(list.last().unwrap(), "fallback-0");
        }
    }

    /// WCAG 2.1 contrast ratio between two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn luminance(c: Color32) -> f32 {
            let channel = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        }
        let (high, low) = (
            luminance(a).max(luminance(b)),
            luminance(a).min(luminance(b)),
        );
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn button_text_on_the_accent_meets_wcag_aa() {
        // ACCENT_INK, not white: ACCENT (#6EA8FE) is light enough that
        // white text on it fails AA (checked below, kept as a regression
        // guard against reintroducing white as the button text colour).
        assert!(contrast(ACCENT_INK, ACCENT) >= 4.5);
        assert!(contrast(Color32::WHITE, ACCENT) < 4.5);
    }

    #[test]
    fn the_type_scale_is_readable_and_headings_stand_out() {
        let mut style = egui::Style::default();
        apply_style(&mut style);
        let size = |s: &TextStyle| style.text_styles[s].size;
        assert!(size(&TextStyle::Body) >= 15.0);
        assert!(size(&TextStyle::Small) >= 12.0);
        assert!(size(&TextStyle::Heading) > size(&TextStyle::Body) + 4.0);
        assert!(style.spacing.interact_size.y >= 28.0);
    }

    #[test]
    fn dark_and_light_palettes_both_clear_body_text_contrast() {
        for palette in [&DARK, &LIGHT] {
            assert!(
                contrast(palette.text, palette.chrome) >= 4.5,
                "body text on chrome must clear WCAG AA"
            );
        }
    }

    /// `install()` sets `visuals.override_text_color` globally (every
    /// unstyled text widget reads `palette.text`, same as
    /// `evermore-panel`'s own approach). `primary_button`'s explicit
    /// `RichText::color(ACCENT_INK)` must still win over that override --
    /// otherwise button text silently becomes `palette.text` (light grey)
    /// on the light `ACCENT` fill, unreadable, and neither contrast test
    /// above would catch it since they only check the constants, not
    /// what a real `Style` actually resolves a `RichText`'s colour to.
    /// This goes through egui's own public `WidgetText::into_layout_job`
    /// rather than asserting a precedence rule by reading egui's source,
    /// so it would fail if a future egui version ever changed that
    /// precedence.
    #[test]
    fn primary_buttons_explicit_text_colour_beats_the_global_override() {
        let mut style = egui::Style::default();
        style.visuals.override_text_color = Some(DARK.text);

        let widget_text: egui::WidgetText = egui::RichText::new("Install").color(ACCENT_INK).into();
        let job =
            widget_text.into_layout_job(&style, egui::FontSelection::Default, egui::Align::Center);

        assert_eq!(
            job.sections[0].format.color, ACCENT_INK,
            "the override_text_color must not flatten an explicitly-coloured RichText"
        );
    }
}
