//! Animated card-back compositing: turns a base back image plus its overlay
//! sprites into the frame strip a theme package's `[backs]` entry expects,
//! and reports each back's playback timing.
//!
//! Four of the original Windows Solitaire's card backs animate: small
//! overlay bitmaps get blitted onto an otherwise static back image on a
//! timer. [`RECIPES`] is the fixed table of which overlays go where, for
//! each of those four backs, reconstructed from the original program's own
//! animation code: `sol.exe` drives every animated back from one
//! fixed-period 250ms `SetTimer`, and every blit is a plain pixel copy
//! (Windows GDI `SRCCOPY` — no blending, no masking). Two backs (the robot
//! and the castle) loop continuously; two (the palm tree and the poker
//! hand) play a brief four-frame "blink" and then hold their plain back for
//! the rest of a longer fixed period.
//!
//! [`compose_strip`] does the actual pixel work, reusing
//! [`crate::raster::RasterImage`] and [`crate::strip::join`] rather than
//! introducing a second image or strip representation; [`timing_for`]
//! converts a recipe's per-cell tick counts into the `fps`/`durations_ms`
//! shape a theme's `[backs]` entry carries. Wiring this into `extract` (so
//! `--animate` actually emits these backs) is a separate module's job.

use std::collections::HashMap;

use sol_theme::BackLayout;

use crate::raster::RasterImage;
use crate::strip;

/// One overlay placement: `sprite_id`'s bitmap, copied byte-for-byte (a
/// plain overwrite — Windows GDI `SRCCOPY`; nothing in the original blends
/// or masks) onto the base back at `(x, y)`.
pub(crate) struct Blit {
    /// The overlay bitmap resource id (678..=684).
    pub sprite_id: u32,
    /// Destination x, in the base back's own pixel coordinates.
    pub x: u32,
    /// Destination y, in the base back's own pixel coordinates.
    pub y: u32,
}

/// One distinct animation frame: `blits`, applied in order over a fresh
/// copy of the base back, held for `ticks` consecutive ticks of the
/// original's one 250ms animation clock. An empty `blits` slice is a
/// "clean" cell — the base back's own pixels, nothing drawn over them.
pub(crate) struct Cell {
    /// The overlays this cell draws, in draw order (a later blit overwrites
    /// an earlier one wherever they overlap).
    pub blits: &'static [Blit],
    /// How many consecutive 250ms ticks this cell holds before the next one
    /// takes over.
    pub ticks: u32,
}

/// One animated back's full recipe: its card-back resource id and the
/// ordered cells its loop cycles through.
pub(crate) struct Recipe {
    /// The card-back resource id this recipe animates (53..=68).
    pub back_id: u32,
    /// This back's cells, in playback order.
    pub cells: &'static [Cell],
}

/// Provenance: from the original Windows Solitaire `sol.exe`, a 16-bit NE
/// executable, sha256 `24946e9eb05d349ee2c2f47058fda7a5fc9dcae6c1a9cc649c8220e66a2cc16e`.
/// The 4-entry recipe table lives in `DGROUP` at offset `0xCA` (32-byte
/// records), walked by a routine at `seg10:0x4E2`. Playback is timed by one
/// 250ms `SetTimer` at `seg1:0x350`; per-back cell selection is the
/// scheduler at `seg4:0x97A`. Cell coordinates, sprite ids, and tick counts
/// below are the disassembled record fields (Castle's sprite 680 uses its
/// true 26x12 size, not the record's latent w=36 over-read).
///
/// Every animated card back's compositing recipe: which overlay sprites to
/// blit, where, and for how many 250ms ticks each distinct frame holds.
/// Every blit here is a plain overwrite (Windows GDI `SRCCOPY`); none of the
/// four animations blend or mask.
pub(crate) const RECIPES: &[Recipe] = &[
    // Robot (card back 56): chest dial/gauge overlay, sprites 683 and 684
    // (both 24x7), both blitted at (24, 40). Continuous 4-cell loop, one
    // tick each: dial A, dial B, dial A, clean.
    Recipe {
        back_id: 56,
        cells: &[
            Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 24,
                    y: 40,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[Blit {
                    sprite_id: 684,
                    x: 24,
                    y: 40,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 24,
                    y: 40,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[],
                ticks: 1,
            },
        ],
    },
    // Castle (card back 63): bats overlay, sprite 680, blitted at (42, 12).
    // Sprite 680's own bitmap is 26x12 wide; a separate field inside the
    // original's disassembled recipe record additionally claims a rect
    // width of 36 for this one entry, which is wider than the sprite's own
    // pixel data — blitting at that width would read 10 columns past the
    // end of the sprite. This recipe (below and in `KNOWN_SPRITE_DIMS`) uses
    // the sprite's true 26x12 size; there is no field here to get wrong.
    // Continuous 2-cell loop, one tick each: wings up, clean.
    Recipe {
        back_id: 63,
        cells: &[
            Cell {
                blits: &[Blit {
                    sprite_id: 680,
                    x: 42,
                    y: 12,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[],
                ticks: 1,
            },
        ],
    },
    // Palm/Beach (card back 64): sun's-tongue overlay, sprites 681 and 682
    // (both 14x12), both blitted at (47, 1). Blinks once (4 ticks: mouth,
    // tongue out, mouth, clean) then holds the clean cell for the rest of a
    // fixed 200-tick (50s) period — 196 more ticks of holding on top of the
    // clean cell's own 1, for 197 total.
    Recipe {
        back_id: 64,
        cells: &[
            Cell {
                blits: &[Blit {
                    sprite_id: 681,
                    x: 47,
                    y: 1,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[Blit {
                    sprite_id: 682,
                    x: 47,
                    y: 1,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[Blit {
                    sprite_id: 681,
                    x: 47,
                    y: 1,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[],
                ticks: 197,
            },
        ],
    },
    // Poker (card back 65): ace-at-the-cuff overlay, sprites 678 and 679
    // (both 32x22), both blitted at (32, 32). Blinks once (4 ticks: ace out,
    // ace further, ace out, clean) then holds the clean cell for the rest of
    // a fixed 60-tick (15s) period — 56 more ticks of holding on top of the
    // clean cell's own 1, for 57 total.
    Recipe {
        back_id: 65,
        cells: &[
            Cell {
                blits: &[Blit {
                    sprite_id: 678,
                    x: 32,
                    y: 32,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[Blit {
                    sprite_id: 679,
                    x: 32,
                    y: 32,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[Blit {
                    sprite_id: 678,
                    x: 32,
                    y: 32,
                }],
                ticks: 1,
            },
            Cell {
                blits: &[],
                ticks: 57,
            },
        ],
    },
];

/// The recipe for `back_id`, if it names one of [`RECIPES`]'s four animated
/// backs.
pub(crate) fn recipe_for(back_id: u32) -> Option<&'static Recipe> {
    RECIPES.iter().find(|recipe| recipe.back_id == back_id)
}

/// Every sprite id any [`RECIPES`] entry blits, in recipe/cell/blit order
/// (an id used by more than one blit repeats).
pub(crate) fn sprite_ids() -> impl Iterator<Item = u32> {
    RECIPES
        .iter()
        .flat_map(|recipe| recipe.cells.iter())
        .flat_map(|cell| cell.blits.iter())
        .map(|blit| blit.sprite_id)
}

/// Every overlay sprite's true pixel dimensions, `(id, width, height)` — the
/// ground truth [`compose_strip`] validates a caller's sprite images
/// against, independent of any per-recipe rect size the original binary
/// might separately (and, for one entry, wrongly) record.
const KNOWN_SPRITE_DIMS: &[(u32, u32, u32)] = &[
    (678, 32, 22),
    (679, 32, 22),
    (680, 26, 12),
    (681, 14, 12),
    (682, 14, 12),
    (683, 24, 7),
    (684, 24, 7),
];

/// Looks up `sprite_id`'s true `(width, height)` in [`KNOWN_SPRITE_DIMS`],
/// or `None` if `sprite_id` isn't one of the seven overlay sprites any
/// [`RECIPES`] entry uses.
fn known_sprite_dims(sprite_id: u32) -> Option<(u32, u32)> {
    KNOWN_SPRITE_DIMS
        .iter()
        .find(|&&(id, _, _)| id == sprite_id)
        .map(|&(_, width, height)| (width, height))
}

/// Composes `recipe`'s cells into one horizontal frame strip. Each cell
/// starts as a fresh copy of `base`; every [`Blit`] in that cell is then
/// applied over it in order — a later blit plainly overwrites an earlier
/// one wherever their rects overlap (Windows GDI `SRCCOPY`; the original
/// never blends or masks). The finished cells are joined left to right via
/// [`crate::strip::join`].
///
/// # Errors
///
/// Returns `Err` if a blit's `sprite_id` is not a key of `sprites`, if that
/// sprite's actual `(width, height)` does not match the known dimensions
/// for its id, or if the blit's destination rect does not fit inside
/// `base`.
pub(crate) fn compose_strip(
    base: &RasterImage,
    sprites: &HashMap<u32, RasterImage>,
    recipe: &Recipe,
) -> Result<RasterImage, String> {
    let mut frames = Vec::with_capacity(recipe.cells.len());
    for cell in recipe.cells {
        let mut frame = base.clone();
        for blit in cell.blits {
            apply_blit(&mut frame, sprites, blit)?;
        }
        frames.push(frame);
    }
    Ok(strip::join(&frames, BackLayout::Horizontal))
}

/// Validates and applies one [`Blit`] onto `frame` in place — see
/// [`compose_strip`]'s error conditions, which this implements. All pixel
/// indexing here goes through checked bounds arithmetic (`saturating_*`)
/// and `.get`/`.get_mut` (never a direct index), so a malformed sprite or
/// base image is silently left untouched rather than causing a panic.
fn apply_blit(
    frame: &mut RasterImage,
    sprites: &HashMap<u32, RasterImage>,
    blit: &Blit,
) -> Result<(), String> {
    let sprite = sprites.get(&blit.sprite_id).ok_or_else(|| {
        format!(
            "blit references sprite {}, which is missing from the sprite set",
            blit.sprite_id
        )
    })?;

    let (width, height) = known_sprite_dims(blit.sprite_id)
        .filter(|&(known_width, known_height)| {
            known_width == sprite.width && known_height == sprite.height
        })
        .ok_or_else(|| {
            format!(
                "sprite {} is {}x{}, which does not match its known dimensions",
                blit.sprite_id, sprite.width, sprite.height
            )
        })?;

    let fits = width <= frame.width.saturating_sub(blit.x)
        && height <= frame.height.saturating_sub(blit.y);
    if !fits {
        return Err(format!(
            "blit for sprite {} at ({}, {}) sized {width}x{height} does not fit inside the {}x{} base",
            blit.sprite_id, blit.x, blit.y, frame.width, frame.height
        ));
    }

    let src_row_bytes = (sprite.width as usize).saturating_mul(4);
    let dst_row_bytes = (frame.width as usize).saturating_mul(4);
    let row_bytes = (width as usize).saturating_mul(4);
    for row in 0..height {
        let src_start = (row as usize).saturating_mul(src_row_bytes);
        let dst_row = blit.y.saturating_add(row);
        let dst_start = (dst_row as usize)
            .saturating_mul(dst_row_bytes)
            .saturating_add((blit.x as usize).saturating_mul(4));

        let src = sprite
            .pixels
            .get(src_start..src_start.saturating_add(row_bytes));
        let dst = frame
            .pixels
            .get_mut(dst_start..dst_start.saturating_add(row_bytes));
        if let (Some(src), Some(dst)) = (src, dst) {
            dst.copy_from_slice(src);
        }
    }

    Ok(())
}

/// The timing form [`timing_for`] emits for one recipe — mirrors
/// `sol_theme::BackTiming`'s two shapes, so the caller can hand this
/// straight to a `[backs]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmittedTiming {
    /// A uniform playback rate: every cell holds for the same number of
    /// ticks.
    Fps(u32),
    /// One explicit display duration per cell, in milliseconds, in cell
    /// order.
    DurationsMs(Vec<u32>),
}

/// The original's one animation clock: every tick is 250ms (`sol.exe`'s
/// single, fixed-period `SetTimer`), and every animated back's cells are
/// timed in whole ticks of it.
const TICK_MS: u32 = 250;

/// The timing `recipe`'s cells emit: [`EmittedTiming::Fps`] when every cell
/// holds the same number of ticks (rounded to the nearest whole frame per
/// second), else [`EmittedTiming::DurationsMs`] with each cell's own
/// `ticks * 250ms` duration, in cell order.
pub(crate) fn timing_for(recipe: &Recipe) -> EmittedTiming {
    let first_ticks = recipe.cells.first().map_or(1, |cell| cell.ticks);
    let uniform = recipe.cells.iter().all(|cell| cell.ticks == first_ticks);

    if uniform {
        let millis = first_ticks.saturating_mul(TICK_MS).max(1);
        // Round-half-away-from-zero division by hand, `(a + b/2) / b`,
        // matching `round(1000 / millis)` without floating point.
        EmittedTiming::Fps((1000 + millis / 2) / millis)
    } else {
        EmittedTiming::DurationsMs(
            recipe
                .cells
                .iter()
                .map(|cell| cell.ticks.saturating_mul(TICK_MS))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::collections::BTreeSet;

    use super::*;

    // ---- fixtures ----

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RasterImage {
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&color);
        }
        RasterImage {
            width,
            height,
            pixels,
        }
    }

    fn pixel_at(image: &RasterImage, x: u32, y: u32) -> &[u8] {
        let row_bytes = (image.width as usize) * 4;
        let start = (y as usize) * row_bytes + (x as usize) * 4;
        image.pixels.get(start..start + 4).unwrap()
    }

    // ---- invariants: RECIPES itself ----

    #[test]
    fn recipes_is_non_empty() {
        assert!(!RECIPES.is_empty());
    }

    #[test]
    fn every_recipe_back_id_is_within_the_animated_back_range() {
        for recipe in RECIPES {
            assert!((53..=68).contains(&recipe.back_id));
        }
    }

    #[test]
    fn every_blit_sprite_id_is_within_the_known_overlay_range() {
        for recipe in RECIPES {
            for cell in recipe.cells {
                for blit in cell.blits {
                    assert!((678..=684).contains(&blit.sprite_id));
                }
            }
        }
    }

    #[test]
    fn every_cell_holds_for_at_least_one_tick() {
        for recipe in RECIPES {
            for cell in recipe.cells {
                assert!(cell.ticks >= 1);
            }
        }
    }

    #[test]
    fn every_recipe_has_at_least_two_cells() {
        // sol-theme's `BackDef::Strip` requires `frames >= 2`, and extract
        // emits `frames = cells.len()`.
        for recipe in RECIPES {
            assert!(recipe.cells.len() >= 2);
        }
    }

    #[test]
    fn every_blit_fits_inside_a_71x96_card_using_the_true_sprite_dimensions() {
        for recipe in RECIPES {
            for cell in recipe.cells {
                for blit in cell.blits {
                    let (width, height) = known_sprite_dims(blit.sprite_id).unwrap();
                    assert!(blit.x + width <= 71);
                    assert!(blit.y + height <= 96);
                }
            }
        }
    }

    // ---- recipe_for / sprite_ids ----

    #[test]
    fn recipe_for_finds_each_real_back() {
        for back_id in [56, 63, 64, 65] {
            assert_eq!(recipe_for(back_id).unwrap().back_id, back_id);
        }
    }

    #[test]
    fn recipe_for_returns_none_for_a_back_with_no_animation() {
        assert!(recipe_for(53).is_none());
    }

    #[test]
    fn sprite_ids_covers_every_known_overlay_sprite() {
        let ids: BTreeSet<u32> = sprite_ids().collect();
        let expected: BTreeSet<u32> = (678..=684).collect();
        assert_eq!(ids, expected);
    }

    // ---- timing_for ----

    #[test]
    fn robot_and_castle_report_a_uniform_four_fps() {
        assert_eq!(timing_for(recipe_for(56).unwrap()), EmittedTiming::Fps(4));
        assert_eq!(timing_for(recipe_for(63).unwrap()), EmittedTiming::Fps(4));
    }

    #[test]
    fn palm_reports_the_blink_then_hold_durations() {
        assert_eq!(
            timing_for(recipe_for(64).unwrap()),
            EmittedTiming::DurationsMs(vec![250, 250, 250, 49_250])
        );
    }

    #[test]
    fn poker_reports_the_blink_then_hold_durations() {
        assert_eq!(
            timing_for(recipe_for(65).unwrap()),
            EmittedTiming::DurationsMs(vec![250, 250, 250, 14_250])
        );
    }

    #[test]
    fn a_uniform_recipe_rounds_the_fps_to_the_nearest_whole_frame() {
        // A synthetic 5-tick-per-cell recipe (1250ms/cell) is not one of the
        // four real backs' rates, but it pins the rounding rule itself:
        // round(1000 / 1250) = round(0.8) = 1, not a mere truncation to 0.
        const RECIPE: Recipe = Recipe {
            back_id: 909,
            cells: &[
                Cell {
                    blits: &[],
                    ticks: 5,
                },
                Cell {
                    blits: &[],
                    ticks: 5,
                },
            ],
        };
        assert_eq!(timing_for(&RECIPE), EmittedTiming::Fps(1));
    }

    // ---- compose_strip: happy paths ----

    #[test]
    fn compose_strip_places_each_cells_blit_and_leaves_the_rest_of_the_base_untouched() {
        const RECIPE: Recipe = Recipe {
            back_id: 900,
            cells: &[
                Cell {
                    blits: &[Blit {
                        sprite_id: 683,
                        x: 5,
                        y: 5,
                    }],
                    ticks: 1,
                },
                Cell {
                    blits: &[Blit {
                        sprite_id: 684,
                        x: 5,
                        y: 5,
                    }],
                    ticks: 1,
                },
            ],
        };
        let base = solid(40, 20, GREEN);
        let mut sprites = HashMap::new();
        sprites.insert(683, solid(24, 7, RED));
        sprites.insert(684, solid(24, 7, BLUE));

        let strip = compose_strip(&base, &sprites, &RECIPE).unwrap();

        assert_eq!(strip.width, 80);
        assert_eq!(strip.height, 20);

        // Cell 0 (strip columns 0..40): the blit rect (5,5)..(29,12) is red;
        // outside it is still the base's green.
        assert_eq!(pixel_at(&strip, 5, 5), RED.as_slice());
        assert_eq!(pixel_at(&strip, 28, 11), RED.as_slice());
        assert_eq!(pixel_at(&strip, 0, 0), GREEN.as_slice());
        assert_eq!(pixel_at(&strip, 39, 19), GREEN.as_slice());

        // Cell 1 (strip columns 40..80, frame-local x = strip x - 40): the
        // blit rect is blue; outside it is still the base's green.
        assert_eq!(pixel_at(&strip, 45, 5), BLUE.as_slice());
        assert_eq!(pixel_at(&strip, 68, 11), BLUE.as_slice());
        assert_eq!(pixel_at(&strip, 40, 0), GREEN.as_slice());
        assert_eq!(pixel_at(&strip, 79, 19), GREEN.as_slice());
    }

    #[test]
    fn a_clean_cell_with_no_blits_is_exactly_the_base() {
        const RECIPE: Recipe = Recipe {
            back_id: 901,
            cells: &[Cell {
                blits: &[],
                ticks: 1,
            }],
        };
        let base = solid(24, 7, YELLOW);
        let sprites = HashMap::new();

        let strip = compose_strip(&base, &sprites, &RECIPE).unwrap();

        assert_eq!(strip, base);
    }

    #[test]
    fn two_blits_in_one_cell_apply_in_order_so_the_later_one_wins_on_overlap() {
        const RECIPE: Recipe = Recipe {
            back_id: 902,
            cells: &[Cell {
                blits: &[
                    Blit {
                        sprite_id: 683,
                        x: 0,
                        y: 0,
                    },
                    Blit {
                        sprite_id: 684,
                        x: 0,
                        y: 0,
                    },
                ],
                ticks: 1,
            }],
        };
        let base = solid(24, 7, GREEN);
        let mut sprites = HashMap::new();
        sprites.insert(683, solid(24, 7, RED));
        sprites.insert(684, solid(24, 7, BLUE));

        let strip = compose_strip(&base, &sprites, &RECIPE).unwrap();

        // Both blits fully cover the (24x7) frame, so the whole result must
        // be blue (684, applied second), with no trace of red (683) left.
        assert_eq!(strip, solid(24, 7, BLUE));
    }

    // ---- compose_strip: error paths ----

    #[test]
    fn a_cell_referencing_a_sprite_missing_from_the_sprite_set_is_an_error() {
        const RECIPE: Recipe = Recipe {
            back_id: 903,
            cells: &[Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 0,
                    y: 0,
                }],
                ticks: 1,
            }],
        };
        let base = solid(40, 20, GREEN);
        let sprites: HashMap<u32, RasterImage> = HashMap::new();

        let error = compose_strip(&base, &sprites, &RECIPE).unwrap_err();
        assert!(error.contains("683"));
    }

    #[test]
    fn a_sprite_whose_actual_dimensions_do_not_match_the_known_dimensions_is_an_error() {
        const RECIPE: Recipe = Recipe {
            back_id: 904,
            cells: &[Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 0,
                    y: 0,
                }],
                ticks: 1,
            }],
        };
        let base = solid(40, 20, GREEN);
        let mut sprites = HashMap::new();
        // 683's known size is 24x7: width right, height wrong (and vice
        // versa below) so both halves of the dimension check are actually
        // exercised, not just "both wrong at once".
        sprites.insert(683, solid(24, 10, RED));

        let error = compose_strip(&base, &sprites, &RECIPE).unwrap_err();
        assert!(error.contains("683"));
    }

    #[test]
    fn a_sprite_whose_width_alone_is_wrong_is_also_an_error() {
        const RECIPE: Recipe = Recipe {
            back_id: 907,
            cells: &[Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 0,
                    y: 0,
                }],
                ticks: 1,
            }],
        };
        let base = solid(40, 20, GREEN);
        let mut sprites = HashMap::new();
        sprites.insert(683, solid(10, 7, RED)); // height right, width wrong

        let error = compose_strip(&base, &sprites, &RECIPE).unwrap_err();
        assert!(error.contains("683"));
    }

    #[test]
    fn a_blit_referencing_an_id_with_no_known_dimensions_is_an_error() {
        const RECIPE: Recipe = Recipe {
            back_id: 905,
            cells: &[Cell {
                blits: &[Blit {
                    sprite_id: 999,
                    x: 0,
                    y: 0,
                }],
                ticks: 1,
            }],
        };
        let base = solid(40, 20, GREEN);
        let mut sprites = HashMap::new();
        sprites.insert(999, solid(5, 5, RED)); // 999 isn't a known overlay id

        let error = compose_strip(&base, &sprites, &RECIPE).unwrap_err();
        assert!(error.contains("999"));
    }

    #[test]
    fn a_blit_rect_that_exceeds_the_base_height_is_an_error_even_when_it_fits_the_width() {
        const RECIPE: Recipe = Recipe {
            back_id: 906,
            cells: &[Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 0,
                    y: 0,
                }],
                ticks: 1,
            }],
        };
        // 683 is 24x7: width 30 comfortably fits, height 5 does not — both
        // halves of the fits check must reject, not just either one.
        let base = solid(30, 5, GREEN);
        let mut sprites = HashMap::new();
        sprites.insert(683, solid(24, 7, RED));

        let error = compose_strip(&base, &sprites, &RECIPE).unwrap_err();
        assert!(error.contains("683"));
    }

    #[test]
    fn a_blit_rect_that_exceeds_the_base_width_is_an_error_even_when_it_fits_the_height() {
        const RECIPE: Recipe = Recipe {
            back_id: 908,
            cells: &[Cell {
                blits: &[Blit {
                    sprite_id: 683,
                    x: 0,
                    y: 0,
                }],
                ticks: 1,
            }],
        };
        // 683 is 24x7: height 20 comfortably fits, width 10 does not.
        let base = solid(10, 20, GREEN);
        let mut sprites = HashMap::new();
        sprites.insert(683, solid(24, 7, RED));

        let error = compose_strip(&base, &sprites, &RECIPE).unwrap_err();
        assert!(error.contains("683"));
    }
}
