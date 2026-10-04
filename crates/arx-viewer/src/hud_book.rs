//! Layout of the player's book (character sheet, quest log) and of notes, from ArxLibertatis's `gui/book/Book.cpp`
//! and `gui/Note.cpp`. Everything is in the original's 640x480 pixels; callers multiply by the interface scale.
//! Nothing here draws or touches the game: drawing is in [`crate::hud_ui`].

use arx_script::{Attribute, Skill};
use bevy::prelude::*;

/// The book occupies this part of the 640x480 screen when it is open.
pub const BOOK_SIZE: Vec2 = Vec2::new(513.0, 313.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookPage {
    Stats,
    Quests,
}

/// Where the book is on a `w` x `h` window at interface scale `s`: centred, but kept clear of the gauges and the
/// icons at the sides (`PlayerBook::updateRect`).
pub fn book_rect(w: f32, h: f32, s: f32) -> Rect {
    let size = BOOK_SIZE * s;
    let mut r = Rect::from_center_size(Vec2::new(w, h) / 2.0, size);
    let (left, right) = ((97.0 + 10.0) * s, w - (640.0 - (97.0 + 513.0)) * s);
    let (top, bottom) = (64.0 * s, h - (480.0 - (64.0 + 313.0)) * s);
    if r.min.x < left {
        r = Rect::from_corners(r.min + Vec2::X * (left - r.min.x), r.max + Vec2::X * (left - r.min.x));
    }
    if r.max.x > right {
        r = Rect::from_corners(r.min + Vec2::X * (right - r.max.x), r.max + Vec2::X * (right - r.max.x));
    }
    let dy = (top + bottom) / 2.0 - r.center().y;
    Rect::from_corners(r.min + Vec2::Y * dy, r.max + Vec2::Y * dy)
}

/// Top-left of attribute icon `a` (32x32), relative to the book.
pub fn attribute_icon(a: Attribute) -> Vec2 {
    Vec2::new(282.0 + 49.0 * a_index(a) as f32, 31.0)
}

/// Where an attribute's value is centred.
pub fn attribute_value(a: Attribute) -> Vec2 {
    Vec2::new([294.0, 343.0, 393.0, 441.0][a_index(a)], 63.0)
}

fn a_index(a: Attribute) -> usize {
    Attribute::ALL.iter().position(|&x| x == a).expect("all attributes are listed")
}

/// Top-left of skill icon `k` (32x32): three rows of three.
pub fn skill_icon(k: Skill) -> Vec2 {
    let i = k as usize;
    Vec2::new([293.0, 356.0, 419.0][i % 3], [113.0, 166.0, 220.0][i / 3])
}

pub fn skill_value(k: Skill) -> Vec2 {
    let i = k as usize;
    Vec2::new([305.0, 369.0, 433.0][i % 3], [144.0, 198.0, 253.0][i / 3])
}

/// The six derived numbers of the left page, each with where it is centred and the area that explains it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Derived {
    ArmorClass,
    ResistMagic,
    ResistPoison,
    Life,
    Mana,
    Damage,
}

impl Derived {
    pub const ALL: [Derived; 6] = [Derived::ArmorClass, Derived::ResistMagic, Derived::ResistPoison, Derived::Life, Derived::Mana, Derived::Damage];

    pub fn value_at(self) -> Vec2 {
        let (x, y) = match self {
            Derived::ArmorClass => (54.0, 90.0),
            Derived::ResistMagic => (54.0, 150.0),
            Derived::ResistPoison => (54.0, 210.0),
            Derived::Life => (227.0, 90.0),
            Derived::Mana => (227.0, 150.0),
            Derived::Damage => (227.0, 210.0),
        };
        Vec2::new(x, y)
    }

    /// The 32x45 area whose mouse-over explains the number.
    pub fn area(self) -> Rect {
        let (x, y) = match self {
            Derived::ArmorClass => (41.0, 62.0),
            Derived::ResistMagic => (41.0, 120.0),
            Derived::ResistPoison => (41.0, 178.0),
            Derived::Life => (211.0, 62.0),
            Derived::Mana => (211.0, 120.0),
            Derived::Damage => (211.0, 178.0),
        };
        Rect::new(x, y, x + 32.0, y + 45.0)
    }

    /// Localisation key of the explanation.
    pub fn help_key(self) -> &'static str {
        match self {
            Derived::ArmorClass => "system_charsheet_ac",
            Derived::ResistMagic => "system_charsheet_res_magic",
            Derived::ResistPoison => "system_charsheet_res_poison",
            Derived::Life => "system_charsheet_hp",
            Derived::Mana => "system_charsheet_mana",
            Derived::Damage => "system_charsheet_damage",
        }
    }
}

pub fn attribute_help_key(a: Attribute) -> &'static str {
    match a {
        Attribute::Strength => "system_charsheet_strength",
        Attribute::Mind => "system_charsheet_intel",
        Attribute::Dexterity => "system_charsheet_dex",
        Attribute::Constitution => "system_charsheet_consti",
    }
}

pub fn skill_help_key(k: Skill) -> &'static str {
    match k {
        Skill::Stealth => "system_charsheet_stealth",
        Skill::Mecanism => "system_charsheet_mecanism",
        Skill::Intuition => "system_charsheet_intuition",
        Skill::EtheralLink => "system_charsheet_etheral_link",
        Skill::ObjectKnowledge => "system_charsheet_objknoledge",
        Skill::Casting => "system_charsheet_casting",
        Skill::CloseCombat => "system_charsheet_closecombat",
        Skill::Projectile => "system_charsheet_projectile",
        Skill::Defense => "system_charsheet_defense",
    }
}

/// Texture (below `graph/interface/book/character_sheet/buttons_carac/`) drawn over an icon while it is pressed.
pub fn attribute_pressed_icon(a: Attribute) -> &'static str {
    match a {
        Attribute::Strength => "icone_strenght",
        Attribute::Mind => "icone_intel",
        Attribute::Dexterity => "icone_dext",
        Attribute::Constitution => "icone_constit",
    }
}

pub fn skill_pressed_icon(k: Skill) -> &'static str {
    match k {
        Skill::Stealth => "icone_stealth",
        Skill::Mecanism => "icone_mecanism",
        Skill::Intuition => "icone_intuition",
        Skill::EtheralLink => "icone_etheral_link",
        Skill::ObjectKnowledge => "icone_obj_knowledge",
        Skill::Casting => "icone_casting",
        Skill::CloseCombat => "icone_close_combat",
        Skill::Projectile => "icone_projectile",
        Skill::Defense => "icone_defense",
    }
}

/// The bookmarks along the top edge: top-left of the one at `slot` (0 = character sheet, 2 = map, 3 = quests).
pub fn bookmark(slot: usize) -> Vec2 {
    Vec2::new(119.0 + 32.0 * slot as f32, -4.0)
}

/// The kinds of readable things (`Note::Type`): the background grows with the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    Notice,
    Small,
    Big,
    Book,
    Quests,
}

impl NoteKind {
    /// Background bitmap name below `graph/interface/book/`.
    pub fn background(self) -> &'static str {
        match self {
            NoteKind::Notice => "notice",
            NoteKind::Small => "bignote",
            NoteKind::Big => "very_bignote",
            NoteKind::Book => "ingame_books",
            NoteKind::Quests => "questbook",
        }
    }

    /// Size of the background bitmap.
    pub fn size(self) -> Vec2 {
        match self {
            NoteKind::Notice => Vec2::new(283.0, 315.0),
            NoteKind::Small => Vec2::new(315.0, 283.0),
            NoteKind::Big => Vec2::new(512.0, 313.0),
            NoteKind::Book | NoteKind::Quests => BOOK_SIZE,
        }
    }

    /// Where the text starts and ends inside the (left page of the) background.
    pub fn text_area(self) -> Rect {
        let size = self.size();
        match self {
            NoteKind::Notice => Rect::new(50.0, 50.0, size.x - 50.0, size.y - 50.0),
            NoteKind::Small => Rect::new(30.0, 30.0, size.x - 30.0, size.y - 40.0),
            NoteKind::Big => Rect::new(40.0, 40.0, size.x * 0.5 - 10.0, size.y - 40.0),
            NoteKind::Book => Rect::new(40.0, 20.0, size.x * 0.5 - 10.0, size.y - 40.0),
            NoteKind::Quests => Rect::new(40.0, 30.0, size.x * 0.5 - 10.0, size.y - 45.0),
        }
    }

    /// How many pages the kind holds.
    pub fn max_pages(self) -> usize {
        match self {
            NoteKind::Notice | NoteKind::Small => 1,
            NoteKind::Big => 2,
            NoteKind::Book | NoteKind::Quests => usize::MAX,
        }
    }

    fn bigger(self) -> NoteKind {
        match self {
            NoteKind::Notice => NoteKind::Small,
            NoteKind::Small => NoteKind::Big,
            _ => NoteKind::Book,
        }
    }
}

impl From<arx_script::NoteKind> for NoteKind {
    fn from(k: arx_script::NoteKind) -> Self {
        match k {
            arx_script::NoteKind::Note => NoteKind::Small,
            arx_script::NoteKind::Notice => NoteKind::Notice,
            arx_script::NoteKind::Book => NoteKind::Book,
        }
    }
}

/// Break `text` into lines of at most `columns` characters (words are kept whole; blank lines are kept).
pub fn wrap(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let needed = if line.is_empty() { word.chars().count() } else { line.chars().count() + 1 + word.chars().count() };
            if needed > columns && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// Fill pages of `lines_per_page` lines.
pub fn paginate(text: &str, columns: usize, lines_per_page: usize) -> Vec<String> {
    let lines = wrap(text, columns);
    let mut pages: Vec<String> = lines.chunks(lines_per_page.max(1)).map(|c| c.join("\n").trim_end().to_owned()).collect();
    // Do not leave a blank last page.
    while pages.len() > 1 && pages.last().is_some_and(String::is_empty) {
        pages.pop();
    }
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

/// What a note looks like once its text is laid out: the kind (bumped up while the text does not fit) and its pages.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteLayout {
    pub kind: NoteKind,
    pub pages: Vec<String>,
}

/// Lay `text` out for a note of `kind` with letters about `char_width` x `line_height` pixels (unscaled), moving to
/// bigger notes until it fits, as `Note::allocate` does.
pub fn layout_note(text: &str, mut kind: NoteKind, char_width: f32, line_height: f32) -> NoteLayout {
    loop {
        let area = kind.text_area();
        let columns = (area.width() / char_width).floor().max(1.0) as usize;
        let lines = (area.height() / line_height).floor().max(1.0) as usize;
        let pages = paginate(text, columns, lines);
        if pages.len() <= kind.max_pages() {
            return NoteLayout { kind, pages };
        }
        kind = kind.bigger();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_book_is_centred_between_the_gauges_and_the_icons() {
        // A 640x480 screen at scale 1: the original's own place for the book.
        let r = book_rect(640.0, 480.0, 1.0);
        assert!((r.min.x - 97.0).abs() < 1e-3 && (r.min.y - 64.0).abs() < 1e-3, "the original's own place: {r:?}");
        assert!((r.center().y - (64.0 + 377.0) / 2.0).abs() < 1e-3);
        // A wide window keeps it in the middle.
        let r = book_rect(1920.0, 1080.0, 2.0);
        assert!((r.center().x - 960.0).abs() < 1e-3, "{r:?}");
        assert!((r.width() - 1026.0).abs() < 1e-3);
    }

    #[test]
    fn the_character_sheet_is_laid_out_on_the_original_grid() {
        assert_eq!(attribute_icon(Attribute::Strength), Vec2::new(282.0, 31.0));
        assert_eq!(attribute_icon(Attribute::Constitution), Vec2::new(429.0, 31.0));
        assert_eq!(skill_icon(Skill::Stealth), Vec2::new(293.0, 113.0));
        assert_eq!(skill_icon(Skill::Casting), Vec2::new(419.0, 166.0));
        assert_eq!(skill_icon(Skill::Defense), Vec2::new(419.0, 220.0));
        assert_eq!(skill_value(Skill::ObjectKnowledge), Vec2::new(369.0, 198.0));
        assert_eq!(attribute_value(Attribute::Mind), Vec2::new(343.0, 63.0));
        assert_eq!(Derived::Life.value_at(), Vec2::new(227.0, 90.0));
        assert_eq!(Derived::ResistPoison.area().min, Vec2::new(41.0, 178.0));
        assert_eq!(bookmark(3), Vec2::new(215.0, -4.0));
    }

    #[test]
    fn text_wraps_on_words_and_keeps_paragraphs() {
        assert_eq!(wrap("the quick brown fox", 9), ["the quick", "brown fox"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("supercalifragilistic", 5), ["supercalifragilistic"], "a long word gets its own line");
        let pages = paginate("one two three four five six", 7, 2);
        assert_eq!(pages, vec!["one two
three".to_owned(), "four
five".to_owned(), "six".to_owned()]);
    }

    #[test]
    fn notes_grow_until_the_text_fits() {
        let short = layout_note("End of goblin kingdom.", NoteKind::Notice, 9.0, 21.0);
        assert_eq!((short.kind, short.pages.len()), (NoteKind::Notice, 1));
        let long = "word ".repeat(120);
        let grown = layout_note(&long, NoteKind::Notice, 9.0, 21.0);
        assert_ne!(grown.kind, NoteKind::Notice, "a long text needs a bigger note");
        assert!(grown.pages.len() <= grown.kind.max_pages());
        let huge = layout_note(&"word ".repeat(2000), NoteKind::Small, 9.0, 21.0);
        assert_eq!(huge.kind, NoteKind::Book);
        assert!(huge.pages.len() > 2);
    }
}
