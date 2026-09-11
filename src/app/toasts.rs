use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use eframe::egui;
use crate::session::InstanceId;

#[derive(Clone, Copy)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

struct Toast {
    severity: Severity,
    title: String,
    body: String,
    expires_at: Option<Instant>,
    url: Option<String>,
    target: Option<InstanceId>,
}

impl Toast {
    fn new(severity: Severity, title: impl Into<String>, body: impl Into<String>) -> Self {
        let expires_in = match severity {
            // Errors stay up until dismissed via their ✕ button.
            // When logging to file gets added an open log button could also be added
            Severity::Error => None,
            Severity::Warning => Some(12),
            Severity::Info => Some(4),
        };
        Toast {
            severity,
            title: title.into(),
            body: body.into(),
            expires_at: expires_in.map(|secs| Instant::now() + Duration::from_secs(secs)),
            url: None,
            target: None,
        }
    }

    fn alive(&self, now: Instant) -> bool {
        self.expires_at.is_none_or(|at| at > now)
    }

    fn draw(&self, ui: &mut egui::Ui, max_width: f32) -> (egui::Response, bool) {
        let accent = match self.severity {
            Severity::Info => egui::Color32::DARK_GRAY,
            Severity::Warning => egui::Color32::ORANGE,
            Severity::Error => egui::Color32::RED,
        };
        let mut dismissed = false;
        let response = ui
            .scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                egui::Frame::window(ui.style())
                    .stroke(egui::Stroke::new(1.5, accent))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(max_width);
                        ui.style_mut().interaction.selectable_labels = false;
                        let title = egui::RichText::new(&self.title).strong();
                        let title = match self.severity {
                            Severity::Info => title.color(egui::Color32::from_gray(180)),
                            Severity::Warning | Severity::Error => title.color(accent),
                        };
                        if self.expires_at.is_none() {
                            ui.horizontal(|ui| {
                                ui.label(title);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        dismissed = ui.small_button("❌").clicked();
                                    },
                                );
                            });
                        } else {
                            ui.label(title);
                        }
                        if !self.body.is_empty() {
                            ui.add_space(2.0);
                            ui.weak(&self.body);
                        }
                    });
            })
            .response;
        (response, dismissed)
    }
}

#[derive(Clone)]
pub struct Toasts {
    items: Arc<Mutex<Vec<Toast>>>,
    ctx: egui::Context
}

impl Toasts {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            items: Default::default(),
            ctx,
        }
    }

    pub fn push(&self, severity: Severity, title: impl Into<String>, body: impl Into<String>) {
        self.items.lock().unwrap().push(Toast::new(severity, title, body));

        self.ctx.request_repaint();
    }

    /// A toast pinned to `target` session tile.
    pub fn push_for(
        &self,
        target: InstanceId,
        severity: Severity,
        title: impl Into<String>,
        body: impl Into<String>,
    ) {
        self.items
            .lock()
            .unwrap()
            .push(Toast { target: Some(target), ..Toast::new(severity, title, body) });

        self.ctx.request_repaint();
    }

    /// A toast that opens `url` when clicked.
    pub fn push_linked(
        &self,
        severity: Severity,
        title: impl Into<String>,
        body: impl Into<String>,
        url: impl Into<String>,
    ) {
        self.items.lock().unwrap().push(Toast {
            url: Some(url.into()),
            expires_at: Some(Instant::now() + Duration::from_secs(12)),
            ..Toast::new(severity, title, body)
        });

        self.ctx.request_repaint();
    }

    pub fn show(&self, ctx: &egui::Context) {
        let mut items = self.items.lock().unwrap();
        let now = Instant::now();
        items.retain(|toast| toast.alive(now));
        if items.is_empty() {
            return;
        }

        egui::Area::new(egui::Id::new("global_toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                Self::show_stack(&mut items, ui, 320.0, None);
            });

        if let Some(at) = items.iter().filter_map(|toast| toast.expires_at).min() {
            ctx.request_repaint_after(at.saturating_duration_since(now));
        }
    }

    pub fn show_for_instance(&self, ctx: &egui::Context, id: InstanceId, rect: egui::Rect) {
        let mut items = self.items.lock().unwrap();
        let now = Instant::now();
        if !items.iter().any(|toast| toast.target == Some(id) && toast.alive(now)) {
            return;
        }

        egui::Area::new(egui::Id::new("instance_toast").with(id))
            .fixed_pos(rect.center_top() + egui::vec2(0.0, 8.0))
            .pivot(egui::Align2::CENTER_TOP)
            .constrain_to(rect)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.shrink_clip_rect(rect);
                ui.spacing_mut().item_spacing.y = 6.0;
                let max_width = (rect.width() - 16.0).min(320.0);
                Self::show_stack(&mut items, ui, max_width, Some(id));
            });

        if let Some(at) = items
            .iter()
            .filter(|toast| toast.target == Some(id) && toast.alive(now))
            .filter_map(|toast| toast.expires_at)
            .min()
        {
            ctx.request_repaint_after(at.saturating_duration_since(now));
        }
    }

    fn show_stack(
        items: &mut Vec<Toast>,
        ui: &mut egui::Ui,
        max_width: f32,
        target: Option<InstanceId>,
    ) {
        let now = Instant::now();
        let mut dismiss = None;
        for (i, toast) in items.iter().enumerate() {
            if toast.target != target || !toast.alive(now) {
                continue;
            }
            let (mut response, dismissed) = toast.draw(ui, max_width);
            if dismissed {
                dismiss = Some(i);
            }
            if toast.url.is_some() {
                response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                if let Some(url) = &toast.url {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
                if toast.expires_at.is_some() {
                    dismiss = Some(i);
                }
            }
        }
        if let Some(i) = dismiss {
            items.remove(i);
        }
    }

    // Remove targets on instance toasts when the session is over so they show
    // in the global stack instead of waiting on tiles that will never draw again.
    pub fn release_targets(&self) {
        for toast in self.items.lock().unwrap().iter_mut() {
            toast.target = None;
        }
    }
}
