use eframe::egui;

#[derive(Ord, PartialEq, Eq)]
pub struct WindowPostion {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}
impl PartialOrd for WindowPostion {
    fn partial_cmp(&self, other: &Self) -> std::option::Option<std::cmp::Ordering> {
        if self.y > other.y {
            return Some(std::cmp::Ordering::Greater);
        }
        if self.y < other.y {
            return Some(std::cmp::Ordering::Less);
        }

        if self.x > other.x {
            return Some(std::cmp::Ordering::Greater);
        }
        if self.x < other.x {
            return Some(std::cmp::Ordering::Less);
        }

        return Some(std::cmp::Ordering::Equal);
    }
}


#[derive(PartialEq)]
pub enum LayoutType { GameLayout, FlatLayout }

impl std::fmt::Display for LayoutType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LayoutType::GameLayout => write!(f, "Game Layout"),
            LayoutType::FlatLayout => write!(f, "Flat Layout"),
        }
    }
}

pub trait LayoutWindows {
    fn layout(&self, window_count: u32, display_width: u32, display_height: u32) -> Vec<WindowPostion>;
    fn display_editor(&mut self, ui: &mut egui::Ui);
    fn clone_box(&self) -> Box<dyn LayoutWindows+Send>;
    fn get_type(&self) -> LayoutType;
}

#[derive(Clone, Copy)]
pub struct GameLayout {
    pub reverse_direction: bool,
    pub ideal_game_width: f32, // like: 16/9
    pub ideal_game_height: f32, // like: 16/9
}
impl LayoutWindows for GameLayout {
    fn layout(&self, window_count: u32, display_width: u32, display_height: u32) -> Vec<WindowPostion> {
        if window_count == 0 {return vec![];}

        // Window counts for width and height
        let (mut w, mut h) = (0, 0);

        // Expand to fill as minimaly required to cover the whole window area
        while w * h < window_count {
            if (display_width * h) as f32 > (display_height * w) as f32 * (self.ideal_game_width/self.ideal_game_height) {
                w += 1;
            } else {
                h += 1;
            }
        }

        // Decrese in height if possible at this width
        while window_count <= w * (h - 1) {
            h -= 1
        }
        // Decrese in width if possible at this height
        while window_count <= (w - 1) * h {
            w -= 1
        }

        let mut windows_output = Vec::new();
        windows_output.reserve(window_count as usize);

        for i in 0..window_count {
            let (col_idx, row_idx) = (
                i / h,
                i % h,
            );

            let rows_fully_fillable = (window_count - 1) % h;

            let is_smaller_row = row_idx > rows_fully_fillable;

            let cur_row_width = (w - (is_smaller_row as u32)).max(1);


            let (window_width, window_height) = (
                (display_width / cur_row_width),
                (display_height / h)
            );

            windows_output.push(
                match self.reverse_direction {
                    false => WindowPostion{
                        x: col_idx * window_width,
                        y: row_idx * window_height,
                        w: window_width,
                        h: window_height,
                    },
                    // Just invert the Y because actualy flipping the logic is harder. 
                    true  => WindowPostion{
                        x: col_idx * window_width,
                        y: display_height-((row_idx+1) * window_height), 
                        w: window_width,
                        h: window_height,
                    }
                }
                
            );
        }

        windows_output.sort();

        windows_output
    }
    fn display_editor(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Reverse direction:");
            ui.checkbox(&mut self.reverse_direction, "");
        });
        ui.horizontal(|ui| {
            ui.label("Ideal ratio");
            ui.add_space(10.0);
            ui.add(
                egui::widgets::DragValue::new(&mut self.ideal_game_width)
                    .range(1..=30)
                    .speed(0.2)
            );
            ui.label("x");
            ui.add(
                egui::widgets::DragValue::new(&mut self.ideal_game_height)
                    .range(1..=30)
                    .speed(0.2)
            );
        });
    }
    fn clone_box(&self) -> Box<dyn LayoutWindows+Send> {
        Box::new(self.clone())
    }
    fn get_type(&self) -> LayoutType {
        LayoutType::GameLayout
    }
}


#[derive(Clone, Copy)]
pub struct FlatLayout {
    pub split_dir_width: bool, // default height.
}
impl LayoutWindows for FlatLayout {
    fn layout(&self, window_count: u32, display_width: u32, display_height: u32) -> Vec<WindowPostion> {
        if window_count == 0 {return vec![];}

        
        let mut windows_output = Vec::new();
        windows_output.reserve(window_count as usize);

        for i in 0..window_count {
            windows_output.push(
                match self.split_dir_width {
                    true => WindowPostion{
                        x: i * display_width / window_count,
                        y: 0,
                        w: display_width / window_count,
                        h: display_height,
                    },
                    false => WindowPostion{
                        x: 0,
                        y: i * display_height / window_count,
                        w: display_width,
                        h: display_height / window_count,
                    },
                }
            );
        }

        windows_output.sort();

        windows_output
    }
    fn display_editor(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Use height instead of width:");
            ui.checkbox(&mut self.split_dir_width, "");
        });
    }
    fn clone_box(&self) -> Box<dyn LayoutWindows+Send> {
        Box::new(self.clone())
    }
    fn get_type(&self) -> LayoutType {
        LayoutType::FlatLayout
    }
}