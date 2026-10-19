use ratatui::{
    style::{Color, Modifier, Style},
    symbols,
};

#[derive(Clone, Copy)]
pub(super) struct Theme {
    pub accent: Color,
    pub good: Color,
    pub warning: Color,
    pub muted: Color,
    pub ascii: bool,
}

impl Theme {
    pub fn new(name: &str, ascii: bool) -> Self {
        let colorblind = matches!(name, "colorblind" | "colorblind-safe" | "colourblind");
        Self {
            accent: if colorblind {
                Color::LightBlue
            } else {
                Color::Cyan
            },
            good: if colorblind {
                Color::LightBlue
            } else {
                Color::Green
            },
            warning: if colorblind {
                Color::LightYellow
            } else {
                Color::Yellow
            },
            muted: Color::Gray,
            ascii,
        }
    }

    pub fn border(self) -> symbols::border::Set {
        if self.ascii {
            symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            }
        } else {
            symbols::border::PLAIN
        }
    }

    pub fn selected(self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    }
}
