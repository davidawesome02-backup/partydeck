use eframe::egui::Ui;

use crate::app::screens::{NavTab, Panels, Screen};
use crate::app::state::AppState;

pub struct HomeScreen;

impl Screen for HomeScreen {
    fn panels(&self, state: &AppState) -> Panels {
        Panels::standard(state).tab(NavTab::Home)
    }

    fn ui(&mut self, _state: &mut AppState, ui: &mut Ui) {
        ui.heading("Welcome to PartyDeck");
        ui.separator();
        ui.label("PartyDeck is in the very early stages of development; as such, you will likely encounter bugs, issues, and strange design decisions.");
        ui.label("For debugging purposes, it's recommended to read terminal output (stdout) for further information on errors.");
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label("Thank you to");
            ui.hyperlink_to("♥Ko-fi", "https://ko-fi.com/wunner");
            ui.label("supporters:");
        });
        ui.label("Framilano, Jayden, Marc, Max Rei");
        ui.horizontal_wrapped(|ui| {
            ui.label("Thank you to");
            ui.hyperlink_to("\u{e624} GitHub", "https://github.com/partydeck/partydeck");
            ui.label("contributors/handler creators:")
        });
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to("@Blahkaey", "https://github.com/Blahkaey");
            ui.hyperlink_to("@blckink", "https://github.com/blckink");
            ui.hyperlink_to("@davidawesome-02", "https://github.com/davidawesome-02");
            ui.hyperlink_to("@felipecrs", "https://github.com/felipecrs");
            ui.hyperlink_to("@framilano", "https://github.com/framilano");
            ui.hyperlink_to("@FrancisBernard34", "https://github.com/FrancisBernard34");
            ui.hyperlink_to("@Rudicito", "https://github.com/Rudicito");
            ui.hyperlink_to("@Tau5", "https://github.com/Tau5");
            ui.hyperlink_to("@Twig6943", "https://github.com/Twig6943");
        });
    }
}
