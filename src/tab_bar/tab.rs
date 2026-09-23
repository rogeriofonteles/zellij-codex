use crate::style;
use crate::{line::tab_separator, LinePart};
use ansi_term::{ANSIString, ANSIStrings};
use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::*;

fn cursors<'a>(
    focused_clients: &'a [ClientId],
    multiplayer_colors: MultiplayerColors,
) -> (Vec<ANSIString<'a>>, usize) {
    // cursor section, text length
    let mut len = 0;
    let mut cursors = vec![];
    for client_id in focused_clients.iter() {
        if let Some(color) = client_id_to_colors(*client_id, multiplayer_colors) {
            cursors.push(style(color.1, color.0).paint(" "));
            len += 1;
        }
    }
    (cursors, len)
}

pub fn render_tab(
    text: String,
    tab: &TabInfo,
    is_alternate_tab: bool,
    palette: Styling,
    separator: &str,
) -> LinePart {
    let focused_clients = tab.other_focused_clients.as_slice();
    let separator_width = separator.width();

    let alternate_tab_color = if is_alternate_tab {
        palette.ribbon_unselected.emphasis_1
    } else {
        palette.ribbon_unselected.background
    };
    let background_color = if tab.active {
        PaletteColor::Rgb((241, 243, 245))
    } else if is_alternate_tab {
        alternate_tab_color
    } else {
        palette.ribbon_unselected.background
    };
    let foreground_color = if tab.is_flashing_bell {
        if tab.active {
            PaletteColor::Rgb((180, 35, 24))
        } else {
            palette.ribbon_unselected.emphasis_3
        }
    } else if tab.active {
        PaletteColor::Rgb((32, 36, 43))
    } else {
        palette.ribbon_unselected.base
    };

    let separator_fill_color = palette.text_unselected.background;
    let left_separator = style(separator_fill_color, background_color).paint(separator);
    let (badge, name) = split_badge(&text, tab.active);
    let base = style(foreground_color, background_color).bold();
    let mut label = vec![base.paint(" ")];
    let mut tab_text_len = name.width() + (separator_width * 2) + 2;
    if let Some((symbol, color)) = badge {
        label.push(
            style(PaletteColor::Rgb(color), background_color)
                .bold()
                .paint(symbol),
        );
        label.push(base.paint(" "));
        tab_text_len += symbol.width() + 1;
    }
    label.push(base.paint(format!("{} ", name)));
    let tab_styled_text = ANSIStrings(&label).to_string();

    let right_separator = style(background_color, separator_fill_color).paint(separator);
    let tab_styled_text = if !focused_clients.is_empty() {
        let (cursor_section, extra_length) =
            cursors(focused_clients, palette.multiplayer_user_colors);
        tab_text_len += extra_length + 2; // 2 for cursor_beginning and cursor_end
        let mut s = String::new();
        let cursor_beginning = style(foreground_color, background_color)
            .bold()
            .paint("[")
            .to_string();
        let cursor_section = ANSIStrings(&cursor_section).to_string();
        let cursor_end = style(foreground_color, background_color)
            .bold()
            .paint("]")
            .to_string();
        s.push_str(&left_separator.to_string());
        s.push_str(&tab_styled_text.to_string());
        s.push_str(&cursor_beginning);
        s.push_str(&cursor_section);
        s.push_str(&cursor_end);
        s.push_str(&right_separator.to_string());
        s
    } else {
        format!("{}{}{}", left_separator, tab_styled_text, right_separator)
    };

    LinePart {
        part: tab_styled_text,
        len: tab_text_len,
        tab_index: Some(tab.position),
    }
}

type Badge = (&'static str, (u8, u8, u8));

fn split_badge(name: &str, selected: bool) -> (Option<Badge>, &str) {
    for (prefix, symbol, color, selected_color) in [
        ("[🟠 ↻] ", "[↻]", (255, 165, 0), (154, 75, 0)),
        ("[🔴 ↻] ", "[↻]", (255, 165, 0), (154, 75, 0)),
        ("[🔴 !] ", "[!]", (255, 77, 77), (180, 35, 24)),
        ("[🔵 ✓] ", "[✓]", (80, 160, 255), (23, 92, 211)),
        ("[🟡 !] ", "[!]", (255, 210, 80), (128, 91, 0)),
        ("[🔴 ✕] ", "[✕]", (255, 77, 77), (180, 35, 24)),
        ("[🟡 Ⅱ] ", "[Ⅱ]", (255, 210, 80), (128, 91, 0)),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return (
                Some((symbol, if selected { selected_color } else { color })),
                rest,
            );
        }
    }
    (None, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_use_rgb_for_active_and_inactive_tabs() {
        for active in [true, false] {
            for (prefix, symbol, inactive_rgb, active_rgb) in [
                ("[🟠 ↻] ", "[↻]", "255;165;0", "154;75;0"),
                ("[🔴 !] ", "[!]", "255;77;77", "180;35;24"),
                ("[🔵 ✓] ", "[✓]", "80;160;255", "23;92;211"),
            ] {
                let tab = TabInfo {
                    active,
                    ..Default::default()
                };
                let rendered = render_tab(
                    format!("{prefix}project"),
                    &tab,
                    false,
                    Styling::default(),
                    "",
                );
                let rgb = if active { active_rgb } else { inactive_rgb };
                assert!(rendered.part.contains(&format!("38;2;{rgb}")));
                if active {
                    assert!(rendered.part.contains("48;2;241;243;245"));
                    assert!(rendered.part.contains("38;2;32;36;43"));
                }
                assert!(rendered.part.contains(symbol));
                assert!(!rendered.part.contains(prefix));
                assert_eq!(rendered.len, " [↻] project ".width());
            }
        }
    }

    #[test]
    fn idle_names_and_click_positions_are_preserved() {
        let tab = TabInfo {
            position: 3,
            ..Default::default()
        };
        let rendered = render_tab("project".into(), &tab, false, Styling::default(), "");
        assert_eq!(rendered.len, 9);
        assert_eq!(get_tab_to_focus(&[rendered], 1, 3), Some(4));
        assert_eq!(split_badge("project", false), (None, "project"));
    }
}

pub fn tab_style(
    mut tabname: String,
    tab: &TabInfo,
    mut is_alternate_tab: bool,
    palette: Styling,
    capabilities: PluginCapabilities,
) -> LinePart {
    let separator = tab_separator(capabilities);

    if tab.is_fullscreen_active {
        tabname.push_str(" (FULLSCREEN)");
    } else if tab.is_sync_panes_active {
        tabname.push_str(" (SYNC)");
    }
    if tab.has_bell_notification || tab.is_flashing_bell {
        tabname.push_str(" [!]");
    }
    // we only color alternate tabs differently if we can't use the arrow fonts to separate them
    if !capabilities.arrow_fonts {
        is_alternate_tab = false;
    }

    render_tab(tabname, tab, is_alternate_tab, palette, separator)
}

pub(crate) fn get_tab_to_focus(
    tab_line: &[LinePart],
    active_tab_idx: usize,
    mouse_click_col: usize,
) -> Option<usize> {
    let clicked_line_part = get_clicked_line_part(tab_line, mouse_click_col)?;
    let clicked_tab_idx = clicked_line_part.tab_index?;
    // tabs are indexed starting from 1 so we need to add 1
    let clicked_tab_idx = clicked_tab_idx + 1;
    if clicked_tab_idx != active_tab_idx {
        return Some(clicked_tab_idx);
    }
    None
}

pub(crate) fn get_clicked_line_part(
    tab_line: &[LinePart],
    mouse_click_col: usize,
) -> Option<&LinePart> {
    let mut len = 0;
    for tab_line_part in tab_line {
        if mouse_click_col >= len && mouse_click_col < len + tab_line_part.len {
            return Some(tab_line_part);
        }
        len += tab_line_part.len;
    }
    None
}
