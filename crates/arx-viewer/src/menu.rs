//! The main menu, which is also the pause menu (`Esc`), its options, and character creation, after the original's
//! `gui/MainMenu.cpp` and `gui/CharacterCreation.cpp`: the entries sit at (370, 100 + 50 n) of the 640x480 screen over
//! `menu_main_background`, a page opens in a window at (20, 25) 321x430 on the left, and a new quest starts on the
//! character sheet of the book with 16 attribute and 18 skill points to hand out. While a menu is open the game's own
//! systems do not run at all, so nothing moves and no script advances.

use crate::audio::Sounds;
use crate::cutscene::Stage;
use crate::hud::Ui;
use crate::hud_book::{self, BookPage};
use crate::hud_ui::{UiAssets, UiFont, interface_scale};
use crate::scripting::Scripting;
use crate::speech::Speech;
use crate::steps::StepSounds;
use crate::{Arx, Fly, Shot};
use arx_formats::locale::Locale;
use arx_formats::mods::Mod;
use bevy::{
    app::AppExit,
    audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, GlobalVolume, PlaybackSettings, SpatialAudioSink, Volume},
    ecs::system::SystemParam,
    platform::collections::HashMap,
    prelude::*,
    ui::widget::NodeImageMode,
    window::{CursorGrabMode, CursorOptions, MonitorSelection, PresentMode, WindowMode},
};
use std::path::PathBuf;

/// One setting of the options page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    Fullscreen,
    Vsync,
    Fov,
    HudScale,
    Subtitles,
    Master,
    Effects,
    SpeechVolume,
    Sensitivity,
    Invert,
}

impl Opt {
    pub const ALL: [Opt; 10] =
        [Opt::Fullscreen, Opt::Vsync, Opt::Fov, Opt::HudScale, Opt::Subtitles, Opt::Master, Opt::Effects, Opt::SpeechVolume, Opt::Sensitivity, Opt::Invert];

    /// On or off (a checkbox) rather than a slider of 0 to 10.
    pub fn is_toggle(self) -> bool {
        matches!(self, Opt::Fullscreen | Opt::Vsync | Opt::Subtitles | Opt::Invert)
    }

    /// Name in the options file.
    fn name(self) -> &'static str {
        match self {
            Opt::Fullscreen => "fullscreen",
            Opt::Vsync => "vsync",
            Opt::Fov => "fov",
            Opt::HudScale => "hud_scale",
            Opt::Subtitles => "subtitles",
            Opt::Master => "master_volume",
            Opt::Effects => "effects_volume",
            Opt::SpeechVolume => "speech_volume",
            Opt::Sensitivity => "mouse_sensitivity",
            Opt::Invert => "invert_mouse",
        }
    }

    /// The game's own text for it, and what to say if the language file has none.
    fn label(self) -> (&'static str, &'static str) {
        match self {
            Opt::Fullscreen => ("system_menus_options_videos_full_screen", "Full screen"),
            Opt::Vsync => ("system_menus_options_video_vsync", "VSync"),
            Opt::Fov => ("system_menus_options_video_fov", "Field of view"),
            Opt::HudScale => ("system_menus_options_interface_hud_scale", "HUD size"),
            Opt::Subtitles => ("system_menus_options_subtitles", "Subtitles"),
            Opt::Master => ("system_menus_options_audio_master_volume", "Master volume"),
            Opt::Effects => ("system_menus_options_audio_effects_volume", "Effects volume"),
            Opt::SpeechVolume => ("system_menus_options_audio_speech_volume", "Speech volume"),
            Opt::Sensitivity => ("system_menus_options_input_mouse_sensitivity", "Mouse sensitivity"),
            Opt::Invert => ("system_menus_options_input_invert_mouse", "Invert mouse"),
        }
    }
}

/// The settings, each 0 to 10 (a toggle is 0 or 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options([u8; 10]);

impl Default for Options {
    fn default() -> Self {
        let mut o = Options([0; 10]);
        for (opt, v) in [(Opt::Vsync, 1), (Opt::Fov, 5), (Opt::HudScale, 10), (Opt::Subtitles, 1), (Opt::Master, 10), (Opt::Effects, 10), (Opt::SpeechVolume, 10), (Opt::Sensitivity, 5)] {
            o.set(opt, v);
        }
        o
    }
}

impl Options {
    pub fn get(&self, opt: Opt) -> u8 {
        self.0[opt as usize]
    }

    pub fn set(&mut self, opt: Opt, value: u8) {
        self.0[opt as usize] = value.min(if opt.is_toggle() { 1 } else { 10 });
    }

    pub fn on(&self, opt: Opt) -> bool {
        self.get(opt) != 0
    }

    fn fraction(&self, opt: Opt) -> f32 {
        self.get(opt) as f32 / 10.0
    }

    /// Vertical field of view of the hero's eyes, degrees: 50 to 100, 75 in the middle.
    pub fn fov_degrees(&self) -> f32 {
        50.0 + 5.0 * self.get(Opt::Fov) as f32
    }

    /// Part of the largest interface that fits the window: 0.5 (the original's default) to 1.
    pub fn hud_scale(&self) -> f32 {
        0.5 + 0.05 * self.get(Opt::HudScale) as f32
    }

    /// Radians turned per pixel of mouse movement: 0.003 in the middle.
    pub fn mouse_speed(&self) -> f32 {
        0.001 + 0.0004 * self.get(Opt::Sensitivity) as f32
    }

    /// Read `name = value` lines; anything unknown or out of range is ignored.
    pub fn parse(text: &str) -> Options {
        let mut o = Options::default();
        for line in text.lines() {
            let Some((name, value)) = line.split_once('=') else { continue };
            if let (Some(opt), Ok(v)) = (Opt::ALL.into_iter().find(|o| o.name() == name.trim()), value.trim().parse::<u8>()) {
                o.set(opt, v);
            }
        }
        o
    }

    pub fn to_text(&self) -> String {
        Opt::ALL.iter().map(|&o| format!("{} = {}\n", o.name(), self.get(o))).collect()
    }
}

/// Where the options are kept between runs: the user's own settings folder, never the repository.
fn options_file() -> Option<PathBuf> {
    arx_formats::mods::config_dir().map(|dir| dir.join("options.cfg"))
}

pub fn load_options() -> Options {
    options_file().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Options::default, |t| Options::parse(&t))
}

fn save_options(o: &Options) {
    if let Some(path) = options_file() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, o.to_text()) {
            eprintln!("cannot save the options to {}: {e}", path.display());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Main,
    Options,
    /// "Start a new quest?", asked when a game is already running.
    ConfirmNew,
    ConfirmQuit,
    /// Character creation.
    Create,
    /// The mods found in the mods folder, to switch on and off.
    Mods,
}

impl Screen {
    pub fn from_name(name: &str) -> Option<Screen> {
        match name {
            "main" => Some(Screen::Main),
            "options" => Some(Screen::Options),
            "create" => Some(Screen::Create),
            "quit" => Some(Screen::ConfirmQuit),
            "mods" => Some(Screen::Mods),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Resume,
    NewQuest,
    Options,
    Quit,
    Back,
    Yes,
    No,
    Toggle(Opt),
    Step(Opt, i8),
    Set(Opt, u8),
    QuickGen,
    Skin,
    Done,
    Mods,
    /// Switch the mod with this number on or off.
    ToggleMod(usize),
    ModsPage(i8),
    /// Keep the choice of mods and start the game again with it.
    ApplyMods,
}

impl Action {
    /// For `--menu-do`.
    pub fn from_name(name: &str) -> Option<Action> {
        Some(match name {
            "resume" => Action::Resume,
            "new" => Action::NewQuest,
            "options" => Action::Options,
            "quit" => Action::Quit,
            "back" => Action::Back,
            "yes" => Action::Yes,
            "no" => Action::No,
            "quickgen" => Action::QuickGen,
            "skin" => Action::Skin,
            "done" => Action::Done,
            "mods" => Action::Mods,
            "togglemod" => Action::ToggleMod(0),
            "applymods" => Action::ApplyMods,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Look {
    Text { text: String, size: f32, align: Align },
    /// An interface bitmap (`graph/interface/<name>`).
    Image(&'static str),
}

/// One thing on a menu screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Where it is drawn (a text's box, which its alignment is within).
    pub at: Rect,
    /// Where the mouse counts as being on it.
    pub hit: Rect,
    pub look: Look,
    pub action: Option<Action>,
    /// Shown but not usable (things this port cannot do yet, or not now).
    pub enabled: bool,
    /// Explanation shown at the top of the screen while the mouse is on it (a localisation key and a fallback).
    pub hint: Option<(&'static str, &'static str)>,
}

/// Text from the game's language file, or the fallback given.
pub struct Labels<'a>(pub &'a Locale);

impl Labels<'_> {
    pub fn get(&self, key: &str, fallback: &str) -> String {
        self.0.get(key).map(str::trim).filter(|t| !t.is_empty()).unwrap_or(fallback).to_owned()
    }
}

/// The font is narrow: about this much of its size per character (Bevy cannot measure text before drawing it).
const CHAR_WIDTH: f32 = 0.47;

fn text_width(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * CHAR_WIDTH
}

fn image(name: &'static str, at: Rect, action: Option<Action>) -> Item {
    Item { at, hit: at, look: Look::Image(name), action, enabled: true, hint: None }
}

/// A line of text in the row `row`, placed by `align`.
fn text(text: String, row: Rect, size: f32, align: Align, action: Option<Action>) -> Item {
    let width = text_width(&text, size).min(row.width());
    let x = match align {
        Align::Left => row.min.x,
        Align::Center => row.center().x - width / 2.0,
        Align::Right => row.max.x - width,
    };
    let hit = Rect::new(x, row.min.y, x + width, row.min.y + size * 1.25);
    Item { at: row, hit, look: Look::Text { text, size, align }, action, enabled: true, hint: None }
}

/// How many mods the mods page lists at a time.
pub const MODS_PER_PAGE: usize = 8;

/// The mods as the menu shows them: what is in the mods folder, which page of it is open, and whether the choice
/// differs from what the game is running with.
pub struct ModList<'a> {
    pub mods: &'a [Mod],
    pub page: usize,
    pub changed: bool,
}

/// Everything on a screen, back to front, for a `w` x `h` window. `started`: a game is running behind the menu.
/// `can_finish`: every point of the new hero is handed out. `book`: where the character sheet is.
#[allow(clippy::too_many_arguments)]
pub fn layout(screen: Screen, started: bool, o: &Options, can_finish: bool, w: f32, h: f32, book: Rect, l: &Labels, mods: &ModList) -> Vec<Item> {
    let (rx, ry) = (w / 640.0, h / 480.0);
    let mut items = Vec::new();
    if screen == Screen::Create {
        items.push(image("book/character_sheet/char_creation_bg", Rect::new(0.0, 0.0, w, h), None));
        let size = 17.0 * ry;
        let y = (book.max.y + 22.0 * ry).min(h - size * 1.5);
        let row = Rect::new(book.min.x, y, book.max.x, y + size * 1.3);
        for (key, fallback, hint, align, action) in [
            ("system_charsheet_button_quickgen", "Quick generation", ("system_charsheet_quickgenerate", "Hand out the points for me"), Align::Left, Action::QuickGen),
            ("system_charsheet_button_skin", "Skin", ("system_charsheet_skin", "Change the hero's face"), Align::Center, Action::Skin),
            ("system_charsheet_button_done", "Done", ("system_charsheet_done", "Hand out every point, then begin"), Align::Right, Action::Done),
        ] {
            let mut item = text(l.get(key, fallback), row, size, align, Some(action));
            item.hint = Some(hint);
            item.enabled = action != Action::Done || can_finish;
            items.push(item);
        }
        return items;
    }

    items.push(image("menus/menu_main_background", Rect::new(0.0, 0.0, w, h), None));
    let size = 27.0 * ry;
    let mut entries = vec![
        ("system_menus_main_resumegame", "Resume game", Some(Action::Resume)),
        ("system_menus_main_newquest", "New quest", Some(Action::NewQuest)),
        ("system_menus_main_editquest", "Load / Save", None),
        ("system_menus_main_options", "Options", Some(Action::Options)),
        ("system_menus_main_credits", "Credits", None),
        ("system_menus_main_quit", "Quit", Some(Action::Quit)),
    ];
    // With something in the mods folder, the menu has an entry for it.
    if !mods.mods.is_empty() {
        entries.insert(4, ("system_menus_main_mods", "Mods", Some(Action::Mods)));
    }
    for (i, (key, fallback, action)) in entries.into_iter().enumerate() {
        let at = Vec2::new(370.0 * rx, (100.0 + 50.0 * i as f32) * ry);
        let mut item = text(l.get(key, fallback), Rect::new(at.x, at.y, w, at.y + size * 1.3), size, Align::Left, action);
        item.enabled = match action {
            Some(Action::Resume) => started,
            Some(_) => true,
            None => false,
        };
        items.push(item);
    }
    if screen == Screen::Main {
        return items;
    }

    // A page: the window on the left.
    let win = Rect::new(20.0 * rx, 25.0 * ry, 341.0 * rx, 455.0 * ry);
    items.push(image("menus/menu_console_background", win, None));
    items.push(image("menus/menu_console_background_border", win, None));
    let (left, right) = (win.min.x + 28.0 * rx, win.max.x - 28.0 * rx);
    match screen {
        Screen::Options => {
            let size = 13.0 * ry;
            let unit = 15.0 * ry;
            for (i, opt) in Opt::ALL.into_iter().enumerate() {
                let y = win.min.y + (40.0 + 31.0 * i as f32) * ry;
                let (key, fallback) = opt.label();
                let label = l.get(key, fallback);
                if opt.is_toggle() {
                    let name = if o.on(opt) { "menus/menu_checkbox_on" } else { "menus/menu_checkbox_off" };
                    items.push(image(name, Rect::new(left, y, left + unit, y + unit), Some(Action::Toggle(opt))));
                    items.push(text(label, Rect::new(left + unit * 1.5, y, right, y + unit), size, Align::Left, Some(Action::Toggle(opt))));
                } else {
                    // Arrow, ten pips, arrow, against the right edge.
                    let x = right - unit * 7.0;
                    items.push(text(label, Rect::new(left, y, x - 4.0 * rx, y + unit), size, Align::Left, None));
                    items.push(image("menus/menu_slider_button_left", Rect::new(x, y, x + unit, y + unit), Some(Action::Step(opt, -1))));
                    for pip in 0..10u8 {
                        let px = x + unit + unit * 0.5 * pip as f32;
                        let name = if pip < o.get(opt) { "menus/menu_slider_on" } else { "menus/menu_slider_off" };
                        items.push(image(name, Rect::new(px, y, px + unit * 0.5, y + unit), Some(Action::Set(opt, pip + 1))));
                    }
                    items.push(image("menus/menu_slider_button_right", Rect::new(x + unit * 6.0, y, x + unit * 7.0, y + unit), Some(Action::Step(opt, 1))));
                }
            }
            let y = win.max.y - 62.0 * ry;
            items.push(image("menus/back", Rect::new(left, y, left + 20.0 * ry, y + 20.0 * ry), Some(Action::Back)));
        }
        Screen::ConfirmNew | Screen::ConfirmQuit => {
            let size = 17.0 * ry;
            let (key, fallback) = if screen == Screen::ConfirmNew {
                ("system_menus_main_newquest_confirm", "Start a new quest?")
            } else {
                ("system_menus_main_editquest_confirm", "Are you sure?")
            };
            let y = win.min.y + 150.0 * ry;
            items.push(text(l.get(key, fallback), Rect::new(left, y, right, y + size * 4.0), size, Align::Center, None));
            let y = win.min.y + 250.0 * ry;
            let row = Rect::new(left + 40.0 * rx, y, right - 40.0 * rx, y + size * 1.3);
            items.push(text(l.get("system_yes", "Yes"), row, size, Align::Left, Some(Action::Yes)));
            items.push(text(l.get("system_no", "No"), row, size, Align::Right, Some(Action::No)));
        }
        Screen::Mods => {
            let (size, small) = (13.0 * ry, 9.5 * ry);
            let unit = 15.0 * ry;
            let pages = mods.mods.len().div_ceil(MODS_PER_PAGE).max(1);
            let page = mods.page.min(pages - 1);
            let y = win.min.y + 30.0 * ry;
            let title = if pages > 1 { format!("{} ({} / {pages})", l.get("system_menus_main_mods", "Mods"), page + 1) } else { l.get("system_menus_main_mods", "Mods") };
            items.push(text(title, Rect::new(left, y, right, y + size * 1.3), 16.0 * ry, Align::Center, None));
            for (row, (index, m)) in mods.mods.iter().enumerate().skip(page * MODS_PER_PAGE).take(MODS_PER_PAGE).enumerate() {
                let y = win.min.y + (62.0 + 38.0 * row as f32) * ry;
                let name = if m.enabled { "menus/menu_checkbox_on" } else { "menus/menu_checkbox_off" };
                items.push(image(name, Rect::new(left, y, left + unit, y + unit), Some(Action::ToggleMod(index))));
                let label = if m.byline().is_empty() { m.name.clone() } else { format!("{}  ({})", m.name, m.byline()) };
                items.push(text(label, Rect::new(left + unit * 1.5, y, right, y + unit), size, Align::Left, Some(Action::ToggleMod(index))));
                // What it is, and how much of the game it touches.
                let what = if m.description.is_empty() { String::new() } else { format!("{}  ", m.description) };
                let about = format!("{what}[{} files, {} replace the game's]", m.files, m.replaces);
                items.push(text(about, Rect::new(left + unit * 1.5, y + size * 1.25, right, y + size * 1.25 + small * 1.3), small, Align::Left, None));
            }
            let y = win.max.y - 62.0 * ry;
            items.push(image("menus/back", Rect::new(left, y, left + 20.0 * ry, y + 20.0 * ry), Some(Action::Back)));
            if pages > 1 {
                let x = left + 40.0 * ry;
                items.push(image("menus/menu_slider_button_left", Rect::new(x, y + 2.0 * ry, x + unit, y + 2.0 * ry + unit), Some(Action::ModsPage(-1))));
                items.push(image("menus/menu_slider_button_right", Rect::new(x + unit * 1.5, y + 2.0 * ry, x + unit * 2.5, y + 2.0 * ry + unit), Some(Action::ModsPage(1))));
            }
            // A changed choice only counts once the game has started again with it.
            let mut apply = text("Apply (restarts the game)".to_owned(), Rect::new(left, y, right, y + size * 1.3), size, Align::Right, Some(Action::ApplyMods));
            apply.enabled = mods.changed;
            items.push(apply);
        }
        Screen::Main | Screen::Create => {}
    }
    items
}

/// The item the mouse is on: the front-most one that does something.
pub fn item_at(items: &[Item], pos: Vec2) -> Option<&Item> {
    items.iter().rev().find(|i| i.action.is_some() && i.hit.contains(pos))
}

#[derive(Resource)]
pub struct Menu {
    pub screen: Option<Screen>,
    pub options: Options,
    /// A game is running behind the menu (so it can be resumed).
    pub started: bool,
    /// The options have been put into effect once.
    applied: bool,
    /// Whether the options are kept between runs (not in screenshot runs, which must not depend on them).
    persistent: bool,
    /// The first "quick generation" gives the average hero, the later ones random ones.
    first_quickgen: bool,
    rng: u32,
    samples: HashMap<String, Option<Handle<AudioSource>>>,
    /// What is in the mods folder (with the choice being made in the menu), and what the game was started with.
    pub mods: Vec<Mod>,
    mods_running: Vec<bool>,
    mods_page: usize,
    /// `--menu-do`: things to click by themselves, one every few frames (the next is last).
    script: Vec<Action>,
    frames: u32,
}

impl Menu {
    pub fn new(screen: Option<Screen>, options: Options, persistent: bool, mut script: Vec<Action>, mods: Vec<Mod>) -> Self {
        script.reverse();
        let mods_running = mods.iter().map(|m| m.enabled).collect();
        let rng = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos()) | 1;
        Menu { started: screen.is_none(), screen, options, applied: false, persistent, first_quickgen: true, rng, samples: HashMap::new(), mods, mods_running, mods_page: 0, script, frames: 0 }
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// The game runs only while no menu is open.
pub fn closed(menu: Res<Menu>) -> bool {
    menu.screen.is_none()
}

/// Character creation uses the book of the interface, so that is drawn and clicked as in the game.
pub fn creating(menu: Res<Menu>) -> bool {
    menu.screen == Some(Screen::Create)
}

#[derive(Component)]
pub struct MenuNode;

/// What the options act on, and what a new hero is made in.
#[derive(SystemParam)]
pub struct Game<'w> {
    s: ResMut<'w, Scripting>,
    ui: ResMut<'w, Ui>,
    speech: ResMut<'w, Speech>,
    sounds: ResMut<'w, Sounds>,
    steps: ResMut<'w, StepSounds>,
    fly: ResMut<'w, Fly>,
    stage: ResMut<'w, Stage>,
    volume: ResMut<'w, GlobalVolume>,
}

#[derive(SystemParam)]
pub struct Gfx<'w> {
    arx: Res<'w, Arx>,
    assets: ResMut<'w, UiAssets>,
    images: ResMut<'w, Assets<Image>>,
    font: Res<'w, UiFont>,
    audio: ResMut<'w, Assets<AudioSource>>,
}

fn apply(o: &Options, game: &mut Game, window: &mut Window) {
    game.volume.volume = Volume::Linear(o.fraction(Opt::Master));
    game.sounds.volume = o.fraction(Opt::Effects);
    game.steps.volume = o.fraction(Opt::Effects);
    game.speech.volume = o.fraction(Opt::SpeechVolume);
    game.speech.subtitles = o.on(Opt::Subtitles);
    game.ui.hud_scale = o.hud_scale();
    game.fly.mouse_speed = o.mouse_speed();
    game.fly.invert_mouse = o.on(Opt::Invert);
    game.stage.2 = o.fov_degrees();
    let mode = if o.on(Opt::Fullscreen) { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    if window.mode != mode {
        window.mode = mode;
    }
    let present = if o.on(Opt::Vsync) { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if window.present_mode != present {
        window.present_mode = present;
    }
}

fn play(menu: &mut Menu, commands: &mut Commands, gfx: &mut Gfx, muted: bool, name: &str) {
    if muted {
        return;
    }
    let handle = menu
        .samples
        .entry(name.to_owned())
        .or_insert_with(|| {
            let bytes = gfx.arx.0.read(&format!("sfx/{name}.wav")).ok()?;
            let pcm = arx_formats::wav::decode(&bytes).ok()?;
            Some(gfx.audio.add(AudioSource { bytes: pcm.to_wav_bytes().into() }))
        })
        .clone();
    if let Some(h) = handle {
        commands.spawn((AudioPlayer::new(h), PlaybackSettings::DESPAWN));
    }
}

/// Start the game again from nothing, on character creation: the level is loaded afresh by a new run of the program.
fn restart(new_quest: bool) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--new-quest").collect();
    let mut command = std::process::Command::new(exe);
    command.args(args);
    if new_quest {
        command.arg("--new-quest");
    }
    command.spawn().is_ok()
}

const TEXT: Color = Color::srgb(232.0 / 255.0, 204.0 / 255.0, 143.0 / 255.0);
const DISABLED: Color = Color::srgb(0.42, 0.38, 0.3);

/// Open and close the menu, act on clicks, and draw the open screen.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut window: Single<(&mut Window, &mut CursorOptions)>,
    shot: Option<Res<Shot>>,
    mut menu: ResMut<Menu>,
    mut game: Game,
    mut gfx: Gfx,
    old: Query<Entity, With<MenuNode>>,
    sinks: Query<&AudioSink>,
    spatial_sinks: Query<&SpatialAudioSink>,
    mut exit: MessageWriter<AppExit>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let (window, cursor) = &mut *window;
    let menu = &mut *menu;
    // Screenshot runs ignore the real keyboard and mouse.
    let live = shot.is_none();
    if !menu.applied {
        menu.applied = true;
        apply(&menu.options, &mut game, window);
        if menu.screen == Some(Screen::Create) {
            game.s.host.player.make_fresh();
        }
    }
    let Some(screen) = menu.screen else {
        // Esc pauses, except where it already means something: putting a note away, or skipping a cutscene.
        if live && keys.just_pressed(KeyCode::Escape) && game.ui.reading.is_none() && !game.s.host.stage.cinemascope {
            menu.screen = Some(Screen::Main);
            for sink in &sinks {
                sink.pause();
            }
            for sink in &spatial_sinks {
                sink.pause();
            }
        }
        return;
    };
    cursor.grab_mode = CursorGrabMode::None;
    // Character creation is drawn by the interface, which has the game's own cursor.
    cursor.visible = screen != Screen::Create;
    if screen == Screen::Create {
        game.ui.book = Some(BookPage::Stats);
        game.ui.creating = true;
        game.ui.cursor_mode = true;
    }
    let muted = game.sounds.muted;
    for name in std::mem::take(&mut game.ui.sfx) {
        play(menu, &mut commands, &mut gfx, muted, name);
    }

    let (w, h) = (window.width(), window.height());
    let book = hud_book::book_rect(w, h, interface_scale(w, h, game.ui.hud_scale));
    let player = &game.s.host.player;
    let can_finish = player.attribute_points == 0 && player.skill_points == 0;
    let mod_list = ModList { mods: &menu.mods, page: menu.mods_page, changed: menu.mods.iter().map(|m| m.enabled).ne(menu.mods_running.iter().copied()) };
    let items = layout(screen, menu.started, &menu.options, can_finish, w, h, book, &Labels(&game.speech.locale), &mod_list);
    let pos = window.cursor_position().filter(|_| live);
    let hovered = pos.and_then(|p| item_at(&items, p)).filter(|i| i.enabled).cloned();

    // Draw. Character creation goes under the interface (the book is drawn on top of it); the rest covers everything.
    let z = if screen == Screen::Create { -10 } else { 100 };
    commands.spawn((
        MenuNode,
        Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        BackgroundColor(Color::BLACK),
        GlobalZIndex(z - 1),
    ));
    let node = |at: Rect| Node {
        position_type: PositionType::Absolute,
        left: Val::Px(at.min.x),
        top: Val::Px(at.min.y),
        width: Val::Px(at.width()),
        height: Val::Px(at.height()),
        ..default()
    };
    let mut hint = None;
    for item in &items {
        let is_hovered = hovered.as_ref().is_some_and(|h| h.hit == item.hit && h.action == item.action);
        if pos.is_some_and(|p| item.hit.contains(p)) && item.hint.is_some() {
            hint = item.hint;
        }
        match &item.look {
            Look::Image(name) => {
                if let Some(tex) = gfx.assets.get(&gfx.arx, &mut gfx.images, name) {
                    let color = if is_hovered {
                        Color::linear_rgb(1.6, 1.6, 1.6)
                    } else if *name == "menus/menu_console_background" {
                        // Darkened, as the original blends it, so that the text on it can be read.
                        Color::srgb(0.4, 0.4, 0.4)
                    } else {
                        Color::WHITE
                    };
                    commands.spawn((MenuNode, node(item.at), ImageNode { image: tex.handle, color, image_mode: NodeImageMode::Stretch, ..default() }, GlobalZIndex(z)));
                }
            }
            Look::Text { text, size, align } => {
                let color = if !item.enabled {
                    DISABLED
                } else if is_hovered {
                    Color::WHITE
                } else {
                    TEXT
                };
                let justify = match align {
                    Align::Left => Justify::Left,
                    Align::Center => Justify::Center,
                    Align::Right => Justify::Right,
                };
                commands.spawn((
                    MenuNode,
                    Text::new(text.clone()),
                    TextFont { font: gfx.font.0.clone().into(), font_size: FontSize::Px(*size), ..default() },
                    TextColor(color),
                    TextLayout::justify(justify),
                    Node { height: Val::Auto, ..node(item.at) },
                    GlobalZIndex(z + 1),
                ));
            }
        }
    }
    // What the button under the mouse does, below the buttons (the book writes its own line at the top).
    if let (Screen::Create, Some((key, fallback))) = (screen, hint) {
        let labels = Labels(&game.speech.locale);
        let mut line = labels.get(key, fallback);
        if key == "system_charsheet_skin" {
            line = format!("{line} ({} / 4)", game.s.host.player.skin + 1);
        }
        let size = 13.0 * h / 480.0;
        let top = items.iter().filter(|i| i.action.is_some()).map(|i| i.hit.max.y).fold(0.0, f32::max) + size * 0.4;
        commands.spawn((
            MenuNode,
            Text::new(line),
            TextFont { font: gfx.font.0.clone().into(), font_size: FontSize::Px(size), ..default() },
            TextColor(TEXT),
            TextLayout::justify(Justify::Center),
            Node { position_type: PositionType::Absolute, left: Val::Px(w * 0.09), top: Val::Px(top), width: Val::Px(w * 0.82), ..default() },
            GlobalZIndex(z + 1),
        ));
    }

    // Act.
    menu.frames += 1;
    let action = if !menu.script.is_empty() {
        if menu.frames % 10 == 0 { menu.script.pop() } else { None }
    } else if live && keys.just_pressed(KeyCode::Escape) {
        Some(Action::Back)
    } else if live && buttons.just_pressed(MouseButton::Left) {
        hovered.and_then(|i| i.action)
    } else {
        None
    };
    let Some(action) = action else { return };
    play(menu, &mut commands, &mut gfx, muted, "menu_click");
    let mut close = false;
    match action {
        Action::Resume => close = menu.started,
        Action::NewQuest if menu.started => menu.screen = Some(Screen::ConfirmNew),
        Action::NewQuest => {
            game.s.host.player.make_fresh();
            menu.first_quickgen = true;
            menu.screen = Some(Screen::Create);
        }
        Action::Options => menu.screen = Some(Screen::Options),
        Action::Mods => menu.screen = Some(Screen::Mods),
        Action::ToggleMod(i) => {
            if let Some(m) = menu.mods.get_mut(i) {
                m.enabled = !m.enabled;
            }
        }
        Action::ModsPage(by) => {
            let pages = menu.mods.len().div_ceil(MODS_PER_PAGE).max(1);
            menu.mods_page = menu.mods_page.saturating_add_signed(by as isize).min(pages - 1);
        }
        Action::ApplyMods => {
            // Mods are read when the game starts, so the choice is kept and the game started again.
            let off = menu.mods.iter().filter(|m| !m.enabled).map(|m| m.id.clone()).collect();
            match arx_formats::mods::write_disabled(&off) {
                Ok(()) if restart(false) => {
                    exit.write(AppExit::Success);
                }
                Ok(()) => {}
                Err(e) => eprintln!("cannot save the choice of mods: {e}"),
            }
        }
        Action::Quit => menu.screen = Some(Screen::ConfirmQuit),
        Action::Back | Action::No => match screen {
            Screen::Main => close = menu.started,
            Screen::Create => {
                game.ui.creating = false;
                game.ui.book = None;
                menu.screen = Some(Screen::Main);
            }
            _ => menu.screen = Some(Screen::Main),
        },
        Action::Yes => {
            if screen == Screen::ConfirmQuit || restart(true) {
                exit.write(AppExit::Success);
            } else {
                menu.screen = Some(Screen::Main);
            }
        }
        Action::Toggle(opt) => menu.options.set(opt, 1 - menu.options.get(opt)),
        Action::Step(opt, by) => menu.options.set(opt, menu.options.get(opt).saturating_add_signed(by)),
        Action::Set(opt, value) => menu.options.set(opt, value),
        Action::QuickGen => {
            let skin = game.s.host.player.skin;
            if menu.first_quickgen {
                menu.first_quickgen = false;
                game.s.host.player.make_average();
            } else {
                let mut dice: Vec<f32> = (0..4096).map(|_| menu.random()).collect();
                game.s.host.player.quick_generate(|| dice.pop().unwrap_or(0.5));
            }
            game.s.host.player.skin = skin;
        }
        Action::Skin => {
            let player = game.s.player;
            let host = &mut game.s.host;
            host.player.skin = (host.player.skin + 1) % 4;
            host.player_entity = Some(player);
            host.refresh_player_model();
        }
        Action::Done => {
            if can_finish {
                // What was handed out here is the hero: it cannot be taken back in the game.
                let p = &mut game.s.host.player;
                p.skills_floor = p.skills;
                game.ui.creating = false;
                game.ui.book = None;
                menu.started = true;
                close = true;
            }
        }
    }
    if matches!(action, Action::Toggle(_) | Action::Step(..) | Action::Set(..)) {
        apply(&menu.options, &mut game, window);
        if menu.persistent {
            save_options(&menu.options);
        }
    }
    if close {
        menu.screen = None;
        if live {
            cursor.grab_mode = CursorGrabMode::Locked;
        }
        cursor.visible = false;
        for sink in &sinks {
            sink.play();
        }
        for sink in &spatial_sinks {
            sink.play();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(screen: Screen, started: bool, o: &Options, can_finish: bool) -> Vec<Item> {
        let locale = Locale::default();
        layout(screen, started, o, can_finish, 1280.0, 960.0, Rect::new(300.0, 200.0, 1000.0, 800.0), &Labels(&locale), &ModList { mods: &[], page: 0, changed: false })
    }

    fn label(item: &Item) -> &str {
        match &item.look {
            Look::Text { text, .. } => text,
            Look::Image(name) => name,
        }
    }

    #[test]
    fn options_survive_being_written_and_read() {
        let mut o = Options::default();
        assert_eq!((o.fov_degrees(), o.hud_scale(), o.on(Opt::Subtitles), o.on(Opt::Fullscreen)), (75.0, 1.0, true, false));
        assert!((o.mouse_speed() - 0.003).abs() < 1e-6);
        o.set(Opt::Master, 3);
        o.set(Opt::Invert, 1);
        o.set(Opt::Fov, 200); // out of range: the top of the slider
        assert_eq!(o.get(Opt::Fov), 10);
        assert_eq!(Options::parse(&o.to_text()), o);
        // Rubbish is ignored, the rest is read.
        let read = Options::parse("nonsense\nmaster_volume = 4\nfov = many\nwho = 3\n");
        assert_eq!((read.get(Opt::Master), read.get(Opt::Fov)), (4, 5));
    }

    #[test]
    fn the_main_menu_is_where_the_original_puts_it() {
        // A window twice 640x480: the entries start at (370, 100) and are 50 apart, doubled.
        let items = screen(Screen::Main, false, &Options::default(), false);
        let entries: Vec<&Item> = items.iter().filter(|i| matches!(i.look, Look::Text { .. })).collect();
        assert_eq!(entries.iter().map(|i| label(i)).collect::<Vec<_>>(), ["Resume game", "New quest", "Load / Save", "Options", "Credits", "Quit"]);
        assert_eq!(entries[0].at.min, Vec2::new(740.0, 200.0));
        assert_eq!(entries[5].at.min, Vec2::new(740.0, 700.0));
        // Nothing to resume yet, and loading and the credits are not there: shown, but they do nothing.
        assert_eq!(entries.iter().map(|i| i.enabled).collect::<Vec<_>>(), [false, true, false, true, false, true]);
        assert_eq!(item_at(&items, Vec2::new(760.0, 310.0)).and_then(|i| i.action), Some(Action::NewQuest));
        assert_eq!(item_at(&items, Vec2::new(100.0, 310.0)).map(|i| i.action), None);
        // With a game behind it, it can be resumed.
        let items = screen(Screen::Main, true, &Options::default(), false);
        assert!(items.iter().find(|i| i.action == Some(Action::Resume)).unwrap().enabled);
    }

    #[test]
    fn the_options_page_has_a_control_for_every_setting() {
        let mut o = Options::default();
        o.set(Opt::Master, 4);
        let items = screen(Screen::Options, true, &o, false);
        for opt in Opt::ALL {
            if opt.is_toggle() {
                assert!(items.iter().any(|i| i.action == Some(Action::Toggle(opt))), "{opt:?}");
            } else {
                assert_eq!(items.iter().filter(|i| matches!(i.action, Some(Action::Set(x, _)) if x == opt)).count(), 10, "{opt:?}");
                assert!(items.iter().any(|i| i.action == Some(Action::Step(opt, -1))) && items.iter().any(|i| i.action == Some(Action::Step(opt, 1))));
            }
        }
        // The slider shows its value: four pips lit; clicking the seventh sets seven.
        let pips: Vec<&Item> = items.iter().filter(|i| matches!(i.action, Some(Action::Set(Opt::Master, _)))).collect();
        assert_eq!(pips.iter().filter(|i| i.look == Look::Image("menus/menu_slider_on")).count(), 4);
        assert_eq!(item_at(&items, pips[6].hit.center()).and_then(|i| i.action), Some(Action::Set(Opt::Master, 7)));
        // Everything is inside the window at (20, 25) 321x430, doubled.
        let win = Rect::new(40.0, 50.0, 682.0, 910.0);
        assert!(items.iter().filter(|i| matches!(i.action, Some(Action::Toggle(_) | Action::Set(..) | Action::Step(..) | Action::Back))).all(|i| win.contains(i.hit.min) && win.contains(i.hit.max)));
        // The main entries stay usable beside it.
        assert!(items.iter().any(|i| i.action == Some(Action::Quit)));
    }

    #[test]
    fn mods_get_a_menu_entry_and_a_page_once_there_are_some() {
        use arx_formats::mods::ModKind;
        let locale = Locale::default();
        let a_mod = |n: usize, enabled: bool| Mod {
            id: format!("mod{n}"),
            name: format!("Mod {n}"),
            author: "me".into(),
            version: String::new(),
            description: "Does things.".into(),
            path: Default::default(),
            kind: ModKind::Folder,
            enabled,
            files: 3,
            replaces: 1,
        };
        let mods: Vec<Mod> = (0..11).map(|n| a_mod(n, n != 1)).collect();
        let show = |screen: Screen, page: usize, changed: bool| layout(screen, true, &Options::default(), false, 1280.0, 960.0, Rect::default(), &Labels(&locale), &ModList { mods: &mods, page, changed });
        // The main menu has one entry more, between the options and the credits; without mods it is as it was.
        let main = show(Screen::Main, 0, false);
        let entries: Vec<&str> = main.iter().filter(|i| matches!(i.look, Look::Text { .. })).map(label).collect();
        assert_eq!(entries, ["Resume game", "New quest", "Load / Save", "Options", "Mods", "Credits", "Quit"]);
        assert!(!screen(Screen::Main, true, &Options::default(), false).iter().any(|i| i.action == Some(Action::Mods)));
        // The page lists eight at a time, each with its switch showing its state.
        let page = show(Screen::Mods, 0, false);
        let switches: Vec<&Item> = page.iter().filter(|i| matches!(i.action, Some(Action::ToggleMod(_))) && matches!(i.look, Look::Image(_))).collect();
        assert_eq!(switches.len(), MODS_PER_PAGE);
        assert_eq!(switches[1].look, Look::Image("menus/menu_checkbox_off"));
        assert_eq!(switches[2].look, Look::Image("menus/menu_checkbox_on"));
        assert!(page.iter().any(|i| label(i) == "Mod 0  (me)") && page.iter().any(|i| label(i).contains("3 files, 1 replace")));
        assert!(page.iter().any(|i| i.action == Some(Action::ModsPage(1))));
        // The second page has the rest, numbered as in the list.
        let rest = show(Screen::Mods, 1, false);
        assert_eq!(rest.iter().filter(|i| matches!(i.action, Some(Action::ToggleMod(n)) if n >= 8) && matches!(i.look, Look::Image(_))).count(), 3);
        // Nothing to apply until the choice differs from what is running.
        let apply = |items: &[Item]| items.iter().find(|i| i.action == Some(Action::ApplyMods)).unwrap().enabled;
        assert!(!apply(&page) && apply(&show(Screen::Mods, 0, true)));
    }

    #[test]
    fn a_hero_is_done_only_when_every_point_is_spent() {
        let items = screen(Screen::Create, false, &Options::default(), false);
        let done = items.iter().find(|i| i.action == Some(Action::Done)).unwrap();
        assert!(!done.enabled);
        // The three buttons are under the book: left, middle, right.
        let x = |a: Action| items.iter().find(|i| i.action == Some(a)).unwrap().hit;
        assert_eq!(x(Action::QuickGen).min.x, 300.0);
        assert_eq!(x(Action::Done).max.x, 1000.0);
        assert!((x(Action::Skin).center().x - 650.0).abs() < 0.01);
        assert!(x(Action::QuickGen).min.y > 800.0);
        assert!(screen(Screen::Create, false, &Options::default(), true).iter().find(|i| i.action == Some(Action::Done)).unwrap().enabled);
    }
}
