//! The two modal dialogs: Game → Options… and Game → Select Game….
//!
//! Split from `ui.rs`, which owns the main window and its event wiring.
//! Each dialog is built once at startup, hidden, and shown modally; both
//! read their edit state out of their own controls and hand it back as one
//! value, so nothing observes a half-edited option set.

use std::cell::RefCell;

use native_windows_gui as nwg;
use sol_engine::ScoringMode;
use sol_theme::CardScaling;

use sol_frontend::app::App;
use sol_frontend::options::EditedOptions;
use sol_frontend::status::seed_digits;
use sol_session::Options;

use crate::ui::build_error;

/// Game → Options…: draw mode, scoring, flags, and the theme/card-back/
/// card-scaling pickers. Theme, back and scaling selections apply to the
/// board immediately (live preview); Cancel restores them, OK commits
/// everything.
pub(crate) struct OptionsDialog {
    pub(crate) window: nwg::Window,
    pub(crate) _label_draw: nwg::Label,
    pub(crate) radio_draw_one: nwg::RadioButton,
    pub(crate) radio_draw_three: nwg::RadioButton,
    pub(crate) _label_scoring: nwg::Label,
    pub(crate) radio_standard: nwg::RadioButton,
    pub(crate) radio_vegas: nwg::RadioButton,
    pub(crate) radio_none: nwg::RadioButton,
    pub(crate) check_timed: nwg::CheckBox,
    pub(crate) check_outline: nwg::CheckBox,
    pub(crate) check_keep_vegas: nwg::CheckBox,
    pub(crate) check_sounds: nwg::CheckBox,
    pub(crate) _label_theme: nwg::Label,
    pub(crate) combo_theme: nwg::ComboBox<String>,
    pub(crate) _label_back: nwg::Label,
    pub(crate) list_back: nwg::ListView,
    image_back: nwg::ImageList,
    /// One entry per back, declaration order — the grid's whole live
    /// state: every frame decoded once at fill time, which image-list
    /// slot (if any) currently shows one of them, and which frame that
    /// slot last drew. `refresh_back_grid` rebuilds this wholesale;
    /// `animate_backs` only ever reads and updates `shown`/`slot`'s
    /// bitmap in place.
    back_slots: RefCell<Vec<BackSlot>>,
    pub(crate) _label_scaling: nwg::Label,
    pub(crate) combo_scaling: nwg::ComboBox<String>,
    pub(crate) _label_hint: nwg::Label,
    pub(crate) ok: nwg::Button,
    pub(crate) cancel: nwg::Button,
}

/// One card back's place in the grid: every frame's decoded bitmap (kept
/// around so an animation tick swaps an image-list slot without decoding
/// a PNG again), the image-list slot those frames cycle through, and the
/// frame last drawn into that slot. `slot` is `None` for a name-only
/// fallback row, or for a theme back that itself declares zero frames —
/// [`BackSheet`](sol_presenter::BackSheet) documents that as a real, if
/// rare, case — either way there is nothing for [`OptionsDialog::animate_backs`]
/// to touch.
struct BackSlot {
    frames: Vec<nwg::Bitmap>,
    slot: Option<i32>,
    shown: u32,
}

/// One theme's card-back preview thumbnails, decoded and ready for the
/// image list: `frames[back][frame]` is that back's frame as a bitmap, in
/// the theme's own back declaration order — one entry per declared back,
/// empty for a back the render could not picture. `cell` is the physical
/// pixel size every image-list slot must be built at.
pub(crate) struct BackPreviews {
    pub(crate) frames: Vec<Vec<nwg::Bitmap>>,
    pub(crate) cell: (u32, u32),
}

/// The picker index for `scaling`: [`OptionsDialog::build`] fills
/// `combo_scaling` with `["Original", "xBRZ"]` in that fixed order, so
/// index `1` means xBRZ.
pub(crate) fn scaling_to_index(scaling: CardScaling) -> usize {
    usize::from(scaling == CardScaling::Xbrz)
}

/// The [`CardScaling`] the `combo_scaling` picker `index` selects: the
/// inverse of [`scaling_to_index`]. Anything other than `Some(1)`
/// (including `None`) is Original.
pub(crate) fn index_to_scaling(index: Option<usize>) -> CardScaling {
    if index == Some(1) {
        CardScaling::Xbrz
    } else {
        CardScaling::Original
    }
}

impl OptionsDialog {
    #[allow(clippy::too_many_lines)] // one linear builder sequence
    pub(crate) fn build(parent: &nwg::Window) -> Result<Self, anyhow::Error> {
        let mut window = nwg::Window::default();
        nwg::Window::builder()
            .flags(nwg::WindowFlags::WINDOW)
            .title("Options")
            .size((470, 672))
            .center(true)
            .parent(Some(parent))
            .build(&mut window)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut label_draw = nwg::Label::default();
        nwg::Label::builder()
            .text("Draw")
            .position((12, 10))
            .size((100, 20))
            .parent(&window)
            .build(&mut label_draw)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut radio_draw_one = nwg::RadioButton::default();
        nwg::RadioButton::builder()
            .text("Draw &one")
            .flags(nwg::RadioButtonFlags::VISIBLE | nwg::RadioButtonFlags::GROUP)
            .position((24, 32))
            .size((180, 22))
            .parent(&window)
            .build(&mut radio_draw_one)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut radio_draw_three = nwg::RadioButton::default();
        nwg::RadioButton::builder()
            .text("Draw &three")
            .position((24, 56))
            .size((180, 22))
            .parent(&window)
            .build(&mut radio_draw_three)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut label_scoring = nwg::Label::default();
        nwg::Label::builder()
            .text("Scoring")
            .position((240, 10))
            .size((100, 20))
            .parent(&window)
            .build(&mut label_scoring)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut radio_standard = nwg::RadioButton::default();
        nwg::RadioButton::builder()
            .text("&Standard")
            .flags(nwg::RadioButtonFlags::VISIBLE | nwg::RadioButtonFlags::GROUP)
            .position((252, 32))
            .size((180, 22))
            .parent(&window)
            .build(&mut radio_standard)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut radio_vegas = nwg::RadioButton::default();
        nwg::RadioButton::builder()
            .text("&Vegas")
            .position((252, 56))
            .size((180, 22))
            .parent(&window)
            .build(&mut radio_vegas)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut radio_none = nwg::RadioButton::default();
        nwg::RadioButton::builder()
            .text("&None")
            .position((252, 80))
            .size((180, 22))
            .parent(&window)
            .build(&mut radio_none)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut check_timed = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .text("Ti&med game")
            .position((12, 112))
            .size((210, 22))
            .parent(&window)
            .build(&mut check_timed)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut check_outline = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .text("Outline &dragging")
            .position((12, 136))
            .size((210, 22))
            .parent(&window)
            .build(&mut check_outline)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut check_keep_vegas = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .text("&Keep Vegas score between games")
            .position((240, 112))
            .size((220, 22))
            .parent(&window)
            .build(&mut check_keep_vegas)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut check_sounds = nwg::CheckBox::default();
        nwg::CheckBox::builder()
            .text("So&unds")
            .position((240, 136))
            .size((210, 22))
            .parent(&window)
            .build(&mut check_sounds)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut label_theme = nwg::Label::default();
        nwg::Label::builder()
            .text("Theme:")
            .position((12, 176))
            .size((80, 22))
            .parent(&window)
            .build(&mut label_theme)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut combo_theme = nwg::ComboBox::default();
        nwg::ComboBox::builder()
            .position((100, 172))
            .size((356, 26))
            .parent(&window)
            .build(&mut combo_theme)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut label_back = nwg::Label::default();
        nwg::Label::builder()
            .text("Card back:")
            .position((12, 208))
            .size((80, 22))
            .parent(&window)
            .build(&mut label_back)
            .map_err(|error| build_error("Options dialog", &error))?;
        // The card-back picker: an icon-mode list view over an image
        // list of live thumbnails, replacing a combo box of theme-author
        // identifiers (`plain`, `weave`, ...) with pictures of the
        // actual artwork. Selection, keyboard navigation and scrolling
        // all come from the control itself. The image list starts at a
        // placeholder size; `OptionsDialog::refresh_back_grid` sizes it
        // to the active theme's own card-back cell size before every
        // fill, so this initial value only has to be non-degenerate.
        let mut image_back = nwg::ImageList::default();
        nwg::ImageList::builder()
            .build(&mut image_back)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut list_back = nwg::ListView::default();
        nwg::ListView::builder()
            .flags(
                nwg::ListViewFlags::VISIBLE
                    | nwg::ListViewFlags::TAB_STOP
                    | nwg::ListViewFlags::SINGLE_SELECTION
                    | nwg::ListViewFlags::ALWAYS_SHOW_SELECTION,
            )
            .position((100, 204))
            .size((356, 300))
            .parent(&window)
            .build(&mut list_back)
            .map_err(|error| build_error("Options dialog", &error))?;
        list_back.set_list_style(nwg::ListViewStyle::Icon);
        list_back.set_image_list(Some(&image_back), nwg::ListViewImageListType::Normal);

        let mut label_scaling = nwg::Label::default();
        nwg::Label::builder()
            .text("Card scaling:")
            .position((12, 520))
            .size((80, 22))
            .parent(&window)
            .build(&mut label_scaling)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut combo_scaling = nwg::ComboBox::default();
        nwg::ComboBox::builder()
            .collection(vec![String::from("Original"), String::from("xBRZ")])
            .position((100, 516))
            .size((356, 26))
            .parent(&window)
            .build(&mut combo_scaling)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut label_hint = nwg::Label::default();
        nwg::Label::builder()
            .text(
                "Theme and card scaling preview live on the board behind this \
                 dialog; card backs preview right here in the grid. Cancel \
                 puts everything back.",
            )
            .position((12, 556))
            .size((444, 36))
            .parent(&window)
            .build(&mut label_hint)
            .map_err(|error| build_error("Options dialog", &error))?;

        let mut ok = nwg::Button::default();
        nwg::Button::builder()
            .text("OK")
            .position((280, 608))
            .size((85, 28))
            .parent(&window)
            .build(&mut ok)
            .map_err(|error| build_error("Options dialog", &error))?;
        let mut cancel = nwg::Button::default();
        nwg::Button::builder()
            .text("Cancel")
            .position((372, 608))
            .size((85, 28))
            .parent(&window)
            .build(&mut cancel)
            .map_err(|error| build_error("Options dialog", &error))?;

        Ok(Self {
            window,
            _label_draw: label_draw,
            radio_draw_one,
            radio_draw_three,
            _label_scoring: label_scoring,
            radio_standard,
            radio_vegas,
            radio_none,
            check_timed,
            check_outline,
            check_keep_vegas,
            check_sounds,
            _label_theme: label_theme,
            combo_theme,
            _label_back: label_back,
            list_back,
            image_back,
            back_slots: RefCell::new(Vec::new()),
            _label_scaling: label_scaling,
            combo_scaling,
            _label_hint: label_hint,
            ok,
            cancel,
        })
    }

    /// Fills every control from the presenter's current options and
    /// the discovered themes.
    pub(crate) fn populate(&self, app: &RefCell<App>) {
        let borrowed = app.borrow();
        let options: Options = borrowed.presenter().options().clone();
        let radio = |on: bool| {
            if on {
                nwg::RadioButtonState::Checked
            } else {
                nwg::RadioButtonState::Unchecked
            }
        };
        let check = |on: bool| {
            if on {
                nwg::CheckBoxState::Checked
            } else {
                nwg::CheckBoxState::Unchecked
            }
        };
        let draw_three = options.draw_mode == sol_engine::DrawMode::Three;
        self.radio_draw_one.set_check_state(radio(!draw_three));
        self.radio_draw_three.set_check_state(radio(draw_three));
        self.radio_standard
            .set_check_state(radio(options.scoring == ScoringMode::Standard));
        self.radio_vegas
            .set_check_state(radio(options.scoring == ScoringMode::Vegas));
        self.radio_none
            .set_check_state(radio(options.scoring == ScoringMode::None));
        self.check_timed.set_check_state(check(options.timed));
        self.check_outline
            .set_check_state(check(options.outline_dragging));
        self.check_keep_vegas
            .set_check_state(check(options.keep_vegas_score));
        self.check_sounds.set_check_state(check(options.sounds));
        self.sync_keep_vegas();

        let ids = borrowed.theme_ids();
        let selected = ids.iter().position(|id| id == borrowed.theme_id());
        self.combo_theme.set_collection(ids);
        self.combo_theme.set_selection(selected);
        drop(borrowed);
        self.refresh_scaling(app);
        // The card-back grid itself is *not* filled here: building its
        // thumbnails means rendering through the render thread, which
        // this dialog has no access to. `crate::ui::Ui::populate_options`
        // (the only caller of `populate`) fills it right after this
        // returns, through `refresh_back_grid` below.
    }

    /// Re-syncs the card-scaling picker to the newly active theme's
    /// recorded choice and PNG-only availability — after a theme switch
    /// (scaling is per-theme) or when the dialog first opens.
    pub(crate) fn refresh_scaling(&self, app: &RefCell<App>) {
        let app = app.borrow();
        self.combo_scaling
            .set_selection(Some(scaling_to_index(app.scaling())));
        self.combo_scaling.set_enabled(app.theme_is_png());
    }

    /// Rebuilds the card-back grid from scratch: one list item per back,
    /// in declaration order, and — when `previews` is `Some` — one
    /// captionless image-list slot per back holding that back's frame at
    /// the presenter's current clock reading (so a freshly filled grid is
    /// never a frame behind the board). `previews` being `None` means no
    /// render was available; every item then falls back to its back's
    /// plain name instead, so the picker stays fully usable rather than
    /// degrading to an empty grid. The same per-item fallback also covers
    /// a back a successful render still couldn't picture (declares zero
    /// frames — see [`BackSlot`]).
    ///
    /// Resizing the image list drops whatever it held, which is exactly
    /// what a theme switch wants, so it is (re)sized before anything is
    /// added to it. Replaces whatever selection the grid had; the active
    /// back is selected again once the rebuild is done.
    ///
    /// Everything needed out of `app` is read up front and the borrow
    /// dropped before any list-view or image-list call below: selecting
    /// the active back at the end can change the list view's selection,
    /// and — unlike a combo box's `CB_SETCURSEL`, which stays silent —
    /// Windows sends `LVN_ITEMCHANGED` for that unconditionally,
    /// synchronously, back into this same dialog's own event loop, which
    /// reaches the selection handler below and borrows `app` again.
    /// Still holding this function's own borrow at that point panics
    /// (`already borrowed`); it must be gone before `select_item` runs.
    pub(crate) fn refresh_back_grid(&self, app: &RefCell<App>, previews: Option<BackPreviews>) {
        let (names, back_index, current_frames) = {
            let app = app.borrow();
            let presenter = app.presenter();
            let current_frames: Vec<u32> = (0..presenter.back_count())
                .map(|back| presenter.back_frame(back))
                .collect();
            (app.back_names(), app.back_index(), current_frames)
        };

        self.list_back.clear();
        let mut slots = self.back_slots.borrow_mut();
        slots.clear();

        if let Some(previews) = previews {
            let (width, height) = previews.cell;
            self.image_back.set_size((
                i32::try_from(width).unwrap_or(i32::MAX),
                i32::try_from(height).unwrap_or(i32::MAX),
            ));
            for (back, frames) in previews.frames.into_iter().enumerate() {
                let frame = current_frames.get(back).copied().unwrap_or(0);
                let bitmap = frames.get(frame as usize).or_else(|| frames.first());
                let slot = bitmap.map(|frame_bitmap| self.push_bitmap(frame_bitmap));
                self.list_back.insert_item(nwg::InsertListViewItem {
                    index: None,
                    column_index: 0,
                    // No caption when a picture is available — the
                    // original showed pictures only; a back the render
                    // still could not picture falls back to its name so
                    // the row is never simply blank.
                    text: if slot.is_none() {
                        names.get(back).cloned()
                    } else {
                        None
                    },
                    image: slot,
                });
                slots.push(BackSlot {
                    frames,
                    slot,
                    shown: if slot.is_some() { frame } else { 0 },
                });
            }
        } else {
            for name in &names {
                self.list_back.insert_item(nwg::InsertListViewItem {
                    index: None,
                    column_index: 0,
                    text: Some(name.clone()),
                    image: None,
                });
            }
        }
        drop(slots);
        self.list_back.select_item(back_index, true);
    }

    /// Appends `bitmap` to the card-back image list and returns its slot
    /// index, leaving that slot written exactly the way an animation tick
    /// writes it.
    ///
    /// Appending is the only way to make a slot exist, but the two
    /// `native-windows-gui` calls do not agree on masking: `add_bitmap`
    /// is `ImageList_AddMasked(himl, hbm, 0)`, which derives a
    /// transparency mask by keying out pure black, while the animation
    /// path's `replace_bitmap` is `ImageList_Replace(himl, i, hbm, NULL)`
    /// and derives none. Card backs are commonly stroked in `#000000`
    /// (both of the shipped default theme's are), so leaving the two
    /// alone would punch a back's outline through to the control's
    /// background on the frame drawn at fill time and paint it solid on
    /// every frame after — a flip as often as the back animates.
    /// Re-writing the freshly appended slot through the very call the
    /// animation uses makes the first frame and all the rest one path.
    ///
    /// No mask is the right side of that disagreement to land on: these
    /// thumbnails are rendered over the list's own opaque background, so
    /// there is nothing in them to key out, and black is artwork.
    fn push_bitmap(&self, bitmap: &nwg::Bitmap) -> i32 {
        let slot = self.image_back.add_bitmap(bitmap);
        self.image_back.replace_bitmap(slot, bitmap);
        slot
    }

    /// One animation tick over the card-back grid: for every back with a
    /// live image-list slot, asks the presenter which frame it should be
    /// showing now — the same clock law the board itself draws by — and,
    /// only when that differs from what the slot last drew, replaces the
    /// slot's bitmap. Invalidates the list once at the end if anything
    /// changed, never per slot. A closed dialog costs exactly one
    /// visibility check: nothing else here runs while it is hidden, and
    /// showing the name-only fallback (every slot `None`) costs one empty
    /// loop.
    ///
    /// `app`'s borrow (through `presenter`) is dropped before any
    /// image-list or list-view call, the same discipline
    /// [`Self::refresh_back_grid`] documents — a replaced bitmap or an
    /// invalidated list never change the *selection*, so today neither
    /// actually reenters this dialog's handlers, but reading every frame
    /// up front costs one small `Vec` and removes the need to reason
    /// about it changing later.
    pub(crate) fn animate_backs(&self, app: &RefCell<App>) {
        if !self.window.visible() {
            return;
        }
        let slot_count = self.back_slots.borrow().len();
        let current_frames: Vec<u32> = {
            let app = app.borrow();
            let presenter = app.presenter();
            (0..slot_count)
                .map(|back| presenter.back_frame(back))
                .collect()
        };

        let mut slots = self.back_slots.borrow_mut();
        let mut changed = false;
        for (back, back_slot) in slots.iter_mut().enumerate() {
            let Some(image_slot) = back_slot.slot else {
                continue;
            };
            let frame = current_frames.get(back).copied().unwrap_or(0);
            if frame == back_slot.shown {
                continue;
            }
            let Some(bitmap) = back_slot.frames.get(frame as usize) else {
                continue;
            };
            self.image_back.replace_bitmap(image_slot, bitmap);
            back_slot.shown = frame;
            changed = true;
        }
        drop(slots);
        if changed {
            self.list_back.invalidate();
        }
    }

    /// "Keep Vegas score" only means something under Vegas scoring.
    pub(crate) fn sync_keep_vegas(&self) {
        self.check_keep_vegas
            .set_enabled(self.radio_vegas.check_state() == nwg::RadioButtonState::Checked);
    }

    /// Reads the dialog back into one options value (OK).
    pub(crate) fn read(&self) -> EditedOptions {
        let checked = |check: &nwg::CheckBox| check.check_state() == nwg::CheckBoxState::Checked;
        let scoring = if self.radio_vegas.check_state() == nwg::RadioButtonState::Checked {
            ScoringMode::Vegas
        } else if self.radio_none.check_state() == nwg::RadioButtonState::Checked {
            ScoringMode::None
        } else {
            ScoringMode::Standard
        };
        EditedOptions {
            draw_three: self.radio_draw_three.check_state() == nwg::RadioButtonState::Checked,
            scoring,
            timed: checked(&self.check_timed),
            outline_dragging: checked(&self.check_outline),
            keep_vegas_score: checked(&self.check_keep_vegas),
            sounds: checked(&self.check_sounds),
        }
    }
}

/// "Select Game…": deal a specific game by its seed number.
pub(crate) struct SelectGameDialog {
    pub(crate) window: nwg::Window,
    pub(crate) _label: nwg::Label,
    pub(crate) input: nwg::TextInput,
    pub(crate) _hint: nwg::Label,
    pub(crate) ok: nwg::Button,
    pub(crate) cancel: nwg::Button,
}

impl SelectGameDialog {
    pub(crate) fn build(parent: &nwg::Window) -> Result<Self, anyhow::Error> {
        let mut window = nwg::Window::default();
        nwg::Window::builder()
            .flags(nwg::WindowFlags::WINDOW)
            .title("Select Game")
            .size((360, 160))
            .center(true)
            .parent(Some(parent))
            .build(&mut window)
            .map_err(|error| build_error("Select Game dialog", &error))?;
        let mut label = nwg::Label::default();
        nwg::Label::builder()
            .text("Game number (0 – 32767):")
            .position((12, 10))
            .size((336, 20))
            .parent(&window)
            .build(&mut label)
            .map_err(|error| build_error("Select Game dialog", &error))?;
        let mut input = nwg::TextInput::default();
        nwg::TextInput::builder()
            .flags(nwg::TextInputFlags::VISIBLE | nwg::TextInputFlags::NUMBER)
            .limit(5)
            .position((12, 34))
            .size((336, 24))
            .parent(&window)
            .build(&mut input)
            .map_err(|error| build_error("Select Game dialog", &error))?;
        let mut hint = nwg::Label::default();
        nwg::Label::builder()
            .text("The same number always deals the same game.")
            .position((12, 66))
            .size((336, 20))
            .parent(&window)
            .build(&mut hint)
            .map_err(|error| build_error("Select Game dialog", &error))?;
        let mut ok = nwg::Button::default();
        nwg::Button::builder()
            .text("OK")
            .position((170, 96))
            .size((85, 28))
            .parent(&window)
            .build(&mut ok)
            .map_err(|error| build_error("Select Game dialog", &error))?;
        let mut cancel = nwg::Button::default();
        nwg::Button::builder()
            .text("Cancel")
            .position((263, 96))
            .size((85, 28))
            .parent(&window)
            .build(&mut cancel)
            .map_err(|error| build_error("Select Game dialog", &error))?;
        Ok(Self {
            window,
            _label: label,
            input,
            _hint: hint,
            ok,
            cancel,
        })
    }

    /// Pre-fills the seed field with the current game's number.
    pub(crate) fn populate(&self, app: &RefCell<App>) {
        self.input.set_text(&seed_digits(app.borrow().presenter()));
    }
}
