use eframe::egui;

/// A window rectangle within a display, in the display's own pixel space.
#[derive(Clone, Copy)]
pub struct WindowPosition {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Which layout an instance display uses. Used for the layout-type combo box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Game,
    Flat,
}

impl std::fmt::Display for LayoutKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutKind::Game => write!(f, "Game Layout"),
            LayoutKind::Flat => write!(f, "Flat Layout"),
        }
    }
}

#[derive(Clone)]
pub enum Layout {
    Game {
        reverse_direction: bool,
        ideal_game_width: f32,
        ideal_game_height: f32,
    },
    Flat {
        split_dir_width: bool,
    },
}

impl Default for Layout {
    fn default() -> Self {
        Layout::Game {
            reverse_direction: false,
            ideal_game_width: 16.0,
            ideal_game_height: 9.0,
        }
    }
}

impl Layout {
    pub fn kind(&self) -> LayoutKind {
        match self {
            Layout::Game { .. } => LayoutKind::Game,
            Layout::Flat { .. } => LayoutKind::Flat,
        }
    }

    pub fn set_kind(&mut self, kind: LayoutKind) {
        if self.kind() == kind {
            return;
        }
        *self = match kind {
            LayoutKind::Game => Layout::default(),
            LayoutKind::Flat => Layout::Flat {
                split_dir_width: false,
            },
        };
    }

    pub fn windows(&self, count: usize, display_width: u32, display_height: u32) -> Vec<WindowPosition> {
        if count == 0 {
            return Vec::new();
        }
        let count = count as u32;
        let mut out = match self {
            Layout::Game {
                reverse_direction,
                ideal_game_width,
                ideal_game_height,
            } => game_windows(
                count,
                display_width,
                display_height,
                *ideal_game_width,
                *ideal_game_height,
                *reverse_direction,
            ),
            Layout::Flat { split_dir_width } => {
                flat_windows(count, display_width, display_height, *split_dir_width)
            }
        };
        out.sort_by_key(|p| (p.y, p.x));
        out
    }

    pub fn editor(&mut self, ui: &mut egui::Ui) {
        match self {
            Layout::Game {
                reverse_direction,
                ideal_game_width,
                ideal_game_height,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Reverse direction:");
                    ui.checkbox(reverse_direction, "");
                });
                ui.horizontal(|ui| {
                    ui.label("Ideal ratio");
                    ui.add_space(10.0);
                    ui.add(egui::DragValue::new(ideal_game_width).range(1..=30).speed(0.2));
                    ui.label("x");
                    ui.add(egui::DragValue::new(ideal_game_height).range(1..=30).speed(0.2));
                });
            }
            Layout::Flat { split_dir_width } => {
                ui.horizontal(|ui| {
                    ui.label("Split horizontal:");
                    ui.checkbox(split_dir_width, "");
                });
            }
        }
    }
}

fn game_windows(
    window_count: u32,
    display_width: u32,
    display_height: u32,
    ideal_game_width: f32,
    ideal_game_height: f32,
    reverse_direction: bool,
) -> Vec<WindowPosition> {
    // Column / row counts.
    let (mut w, mut h) = (0u32, 0u32);

    // Expand to minimally cover the whole area while respecting the ideal cell ratio.
    while w * h < window_count {
        if (display_width * h) as f32
            > (display_height * w) as f32 * (ideal_game_width / ideal_game_height)
        {
            w += 1;
        } else {
            h += 1;
        }
    }
    // Shrink height, then width, if the count still fits.
    while window_count <= w * (h - 1) {
        h -= 1;
    }
    while window_count <= (w - 1) * h {
        w -= 1;
    }

    let mut out = Vec::with_capacity(window_count as usize);
    for i in 0..window_count {
        let (col_idx, row_idx) = (i / h, i % h);

        let rows_fully_fillable = (window_count - 1) % h;
        let is_smaller_row = row_idx > rows_fully_fillable;
        let cur_row_width = (w - (is_smaller_row as u32)).max(1);

        let (window_width, window_height) = (display_width / cur_row_width, display_height / h);

        out.push(match reverse_direction {
            false => WindowPosition {
                x: col_idx * window_width,
                y: row_idx * window_height,
                w: window_width,
                h: window_height,
            },
            // Invert Y rather than flipping the fill logic.
            true => WindowPosition {
                x: col_idx * window_width,
                y: display_height - ((row_idx + 1) * window_height),
                w: window_width,
                h: window_height,
            },
        });
    }
    out
}

fn flat_windows(
    window_count: u32,
    display_width: u32,
    display_height: u32,
    split_dir_width: bool,
) -> Vec<WindowPosition> {
    let mut out = Vec::with_capacity(window_count as usize);
    for i in 0..window_count {
        out.push(match split_dir_width {
            true => WindowPosition {
                x: i * display_width / window_count,
                y: 0,
                w: display_width / window_count,
                h: display_height,
            },
            false => WindowPosition {
                x: 0,
                y: i * display_height / window_count,
                w: display_width,
                h: display_height / window_count,
            },
        });
    }
    out
}
