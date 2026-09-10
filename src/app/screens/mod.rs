mod edit_handler;
mod game;
mod home;
mod instances;
mod profiles;
mod session;
mod settings;

use edit_handler::EditHandlerScreen;
use game::GameScreen;
use home::HomeScreen;
use instances::InstancesScreen;
use profiles::ProfilesScreen;
use session::SessionScreen;
use settings::SettingsScreen;


use eframe::egui;

use super::state::AppState;
use crate::handler::Handler;
use crate::input::PadButton;
use crate::session::Session;

pub enum Route {
    Home,
    Settings,
    Profiles,
    EditHandler(Handler),
    Game,
    Instances,
    Session(Session, Handler),
}

impl Route {
    pub fn build(self, state: &mut AppState) -> Box<dyn Screen> {
        match self {
            Route::Home => Box::new(HomeScreen),
            Route::Settings => Box::new(SettingsScreen::default()),
            Route::Profiles => Box::new(ProfilesScreen::new(state)),
            Route::EditHandler(handler) => Box::new(EditHandlerScreen::new(handler)),
            Route::Game => Box::new(GameScreen),
            Route::Instances => Box::new(InstancesScreen::new(state)),
            Route::Session(session, handler) => Box::new(SessionScreen::new(session, handler)),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum NavTab {
    Home,
    Settings,
    Profiles,
}

/// Which surrounding panels a screen wants around its central content
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Panels {
    pub top: bool,
    /// Which top bar tab shows as selected
    pub tab: Option<NavTab>,
    pub left: bool,
    pub right: bool,
    /// Height of the bottom panel, if the screen wants one
    pub bottom: Option<f32>,
}

impl Panels {
    pub const INFO_HEIGHT: f32 = 100.0;

    pub fn standard(state: &AppState) -> Self {
        Self {
            top: true,
            left: !state.mode.is_lite(),
            ..Self::default()
        }
    }

    pub fn tab(mut self, tab: NavTab) -> Self {
        self.tab = Some(tab);
        self
    }

    pub fn bottom(mut self, height: f32) -> Self {
        self.bottom = Some(height);
        self
    }
}

pub trait Screen {
    /// Main content to be drawn for this screen
    fn ui(&mut self, state: &mut AppState, ui: &mut egui::Ui);

    /// Which surrounding panels this screen wants, none by default.
    fn panels(&self, _state: &AppState) -> Panels {
        Panels::default()
    }

    /// Which monitor this screen fullscreens the window on, none by default
    fn pinned_monitor(&self) -> Option<String> {
        None
    }

    /// Bottom panel content to be drawn for this screen
    fn bottom_panel(&mut self, _state: &mut AppState, _ui: &mut egui::Ui) {}

    /// Gamepad bindings for this screen
    fn handle_gamepad(&mut self, _state: &mut AppState, _presses: &[(usize, PadButton)]) {}

    fn should_capture_ctrl_c(&mut self) -> bool {false}
}
