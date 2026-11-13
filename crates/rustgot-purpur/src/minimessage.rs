//! The part of Adventure's `MiniMessage` format that Purpur's messages use.
//!
//! Colors, decorations, `<reset>` and `<newline>` are rendered. Tags that need more (gradients,
//! hover, click, ...) keep their text unstyled, and unknown tags stay literal text like in
//! `MiniMessage`.

use pumpkin_util::text::{
    TextComponent,
    color::{Color, NamedColor, RGBColor},
};

/// Tags `MiniMessage` knows that are not rendered here; their content is kept without the effect.
const UNRENDERED_TAGS: &[&str] = &[
    "gradient",
    "rainbow",
    "transition",
    "shadow",
    "font",
    "hover",
    "click",
    "insert",
    "insertion",
    "pride",
];

#[derive(Clone, Copy)]
enum Decoration {
    Bold,
    Italic,
    Underlined,
    Strikethrough,
    Obfuscated,
}

#[derive(Clone, Copy)]
enum Effect {
    Color(Color),
    Decoration(Decoration, bool),
    None,
}

fn decoration(name: &str) -> Option<Decoration> {
    Some(match name {
        "bold" | "b" => Decoration::Bold,
        "italic" | "i" | "em" => Decoration::Italic,
        "underlined" | "u" => Decoration::Underlined,
        "strikethrough" | "st" => Decoration::Strikethrough,
        "obfuscated" | "obf" => Decoration::Obfuscated,
        _ => return None,
    })
}

fn color(name: &str) -> Option<Color> {
    if let Some(hex) = name.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let rgb = u32::from_str_radix(hex, 16).ok()?;
        let [_, red, green, blue] = rgb.to_be_bytes();
        return Some(Color::Rgb(RGBColor::new(red, green, blue)));
    }
    let name = match name {
        "grey" => "gray",
        "dark_grey" => "dark_gray",
        name => name,
    };
    NamedColor::try_from(name).ok().map(Color::Named)
}

/// What an opening tag does, or `None` when `MiniMessage` would not treat it as a tag.
fn effect(tag: &str) -> Option<Effect> {
    let (negated, tag) = tag
        .strip_prefix('!')
        .map_or((false, tag), |tag| (true, tag));
    let mut parts = tag.split(':');
    let name = parts.next()?;
    let argument = parts.next();
    if let Some(decoration) = decoration(name) {
        let enabled = !negated && argument != Some("false");
        return Some(Effect::Decoration(decoration, enabled));
    }
    if negated {
        return None;
    }
    if matches!(name, "color" | "colour" | "c") {
        return argument.and_then(color).map(Effect::Color);
    }
    if UNRENDERED_TAGS.contains(&name) {
        return Some(Effect::None);
    }
    color(name).map(Effect::Color)
}

fn styled(text: String, stack: &[(String, Effect)]) -> TextComponent {
    let mut component = TextComponent::text(text);
    let style = &mut component.0.style;
    for (_, effect) in stack {
        match *effect {
            Effect::Color(color) => style.color = Some(color),
            Effect::Decoration(decoration, enabled) => {
                let flag = match decoration {
                    Decoration::Bold => &mut style.bold,
                    Decoration::Italic => &mut style.italic,
                    Decoration::Underlined => &mut style.underlined,
                    Decoration::Strikethrough => &mut style.strikethrough,
                    Decoration::Obfuscated => &mut style.obfuscated,
                };
                *flag = Some(enabled);
            }
            Effect::None => {}
        }
    }
    component
}

/// Parses `input` as `MiniMessage` into a text component.
#[must_use]
pub fn parse(input: &str) -> TextComponent {
    let mut root = TextComponent::text("");
    let mut stack: Vec<(String, Effect)> = Vec::new();
    let mut text = String::new();
    let mut rest = input;

    while let Some(c) = rest.chars().next() {
        if c == '\\' && (rest[1..].starts_with('<') || rest[1..].starts_with('\\')) {
            text.push_str(&rest[1..2]);
            rest = &rest[2..];
            continue;
        }
        let tag_end = rest.find('>');
        let (true, Some(end)) = (c == '<', tag_end) else {
            text.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        let raw = &rest[1..end];
        let tag = raw.to_ascii_lowercase();
        let tag = tag.strip_suffix('/').unwrap_or(&tag);

        let handled = if let Some(name) = tag.strip_prefix('/') {
            stack
                .iter()
                .rposition(|(open, _)| open == name)
                .map(|index| {
                    flush(&mut root, &mut text, &stack);
                    stack.truncate(index);
                })
        } else if matches!(tag, "reset") {
            flush(&mut root, &mut text, &stack);
            stack.clear();
            Some(())
        } else if matches!(tag, "newline" | "br") {
            text.push('\n');
            Some(())
        } else {
            effect(tag).map(|effect| {
                flush(&mut root, &mut text, &stack);
                let name = tag.split(':').next().unwrap_or(tag);
                stack.push((name.trim_start_matches('!').to_owned(), effect));
            })
        };
        if handled.is_some() {
            rest = &rest[end + 1..];
        } else {
            text.push('<');
            rest = &rest[1..];
        }
    }
    flush(&mut root, &mut text, &stack);
    root
}

fn flush(root: &mut TextComponent, text: &mut String, stack: &[(String, Effect)]) {
    if !text.is_empty() {
        root.0.extra.push(styled(std::mem::take(text), stack).0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::text::style::Style;

    fn parts(input: &str) -> Vec<(String, Style)> {
        parse(input)
            .0
            .extra
            .into_iter()
            .map(|part| {
                let style = (*part.style).clone();
                (TextComponent(part).get_text(), style)
            })
            .collect()
    }

    #[test]
    fn purpur_afk_message() {
        let parts = parts("<yellow><italic>Steve is now AFK");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].0, "Steve is now AFK");
        assert_eq!(parts[0].1.color, Some(Color::Named(NamedColor::Yellow)));
        assert_eq!(parts[0].1.italic, Some(true));
    }

    #[test]
    fn closing_and_reset_tags() {
        let parts = parts("<gray>Ram<yellow>:</yellow> 5<reset> plain");
        let texts: Vec<_> = parts.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(texts, ["Ram", ":", " 5", " plain"]);
        assert_eq!(parts[1].1.color, Some(Color::Named(NamedColor::Yellow)));
        assert_eq!(parts[2].1.color, Some(Color::Named(NamedColor::Gray)));
        assert_eq!(parts[3].1.color, None);
    }

    #[test]
    fn hex_negation_and_aliases() {
        let parts = parts("<#ff5555><b>a<!b>b</b><color:dark_grey>c");
        assert_eq!(
            parts[0].1.color,
            Some(Color::Rgb(RGBColor::new(0xff, 0x55, 0x55)))
        );
        assert_eq!(parts[0].1.bold, Some(true));
        assert_eq!(parts[1].1.bold, Some(false));
        assert_eq!(parts[2].1.color, Some(Color::Named(NamedColor::DarkGray)));
    }

    #[test]
    fn unknown_tags_and_escapes_stay_text() {
        let texts: Vec<_> = parts(r"<notatag>a \<red> b < c <gradient:#fff:#000>d</gradient>")
            .into_iter()
            .map(|(text, _)| text)
            .collect();
        assert_eq!(texts.concat(), "<notatag>a <red> b < c d");
    }
}
