pub struct WindowPostion {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub trait LayoutWindows {
    fn layout(&self, count: u32, width: u32, height: u32) -> Vec<WindowPostion>;
}

pub struct GameLayout {
    pub reverse_direction: bool,
    pub ideal_ratio: f32, // like: 16/9
}
impl LayoutWindows for GameLayout {
    fn layout(&self, window_count: u32, display_width: u32, display_height: u32) -> Vec<WindowPostion> {
        // Window counts for width and height
        let (mut w, mut h) = (0, 0);

        // Expand to fill as minimaly required to cover the whole window area
        while w * h < window_count {
            if (display_width * h) as f32 > (display_height * w) as f32 * self.ideal_ratio {
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

        windows_output
    }
}


pub struct FlatLayout {
    split_dir_width: bool, // default height.
}
impl LayoutWindows for FlatLayout {
    fn layout(&self, window_count: u32, display_width: u32, display_height: u32) -> Vec<WindowPostion> {
        let mut windows_output = Vec::new();
        windows_output.reserve(window_count as usize);

        for i in 0..window_count {
            windows_output.push(
                match self.split_dir_width {
                    true => WindowPostion{
                        x: i * display_width / window_count,
                        y: display_height,
                        w: display_width / window_count,
                        h: display_height,
                    },
                    false => WindowPostion{
                        x: display_width,
                        y: i * display_height / window_count,
                        w: display_width,
                        h: display_height / window_count,
                    },
                }
            );
        }

        windows_output
    }
}