//! Look and feel: type scale, spacing, accent colour and font fallbacks.
//!
//! egui follows the system light/dark setting by default (`ThemePreference::
//! System`); this module only adjusts sizes and colours for both themes so the
//! app is comfortable to read and click, and it adds system fonts as fallbacks
//! so publisher-written text in any script shows up instead of empty boxes.
//! Nothing here is bundled: a missing font just means that script stays a box.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

/// The colour of the main action on a screen (Install, Update).
pub const ACCENT: Color32 = Color32::from_rgb(40, 100, 200);

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
    ctx.all_styles_mut(apply_style);
}

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
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(11.0, 5.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.window_margin = egui::Margin::same(14);
    let rounding = egui::CornerRadius::same(6);
    let widgets = &mut style.visuals.widgets;
    widgets.noninteractive.corner_radius = rounding;
    widgets.inactive.corner_radius = rounding;
    widgets.hovered.corner_radius = rounding;
    widgets.active.corner_radius = rounding;
    widgets.open.corner_radius = rounding;
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke.color = Color32::WHITE;
    style.visuals.hyperlink_color = ACCENT;
}

/// The main action on a screen: filled with the accent colour.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).color(Color32::WHITE)).fill(ACCENT)
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
        assert!(contrast(Color32::WHITE, ACCENT) >= 4.5);
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
}
