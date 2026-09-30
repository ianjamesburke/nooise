//! Slow auto-morph: the built-in destinations and the clock that walks
//! between them.
//!
//! `AUTO_STATES` decodes to full `SongState` endpoints at startup, `MorphState`
//! derives leg index and progress purely from the live beat clock so renders
//! stay deterministic, and `MorphWriter` throttles how often the engine's
//! controls and automation are recomputed and stored.

use std::sync::Arc;

use arc_swap::ArcSwap;

#[cfg(test)]
use super::automation::{
    ControlAddress, LfoRoute, LfoShape, ModContext, modulated_control_value_full,
};
use super::module::{ModuleSlotField, module_kind_at};
use super::registry::parse_module_slot_id;
use super::{
    AutomationState, ControlKind, ControlSpec, FluidControls, GRID_BEAT_EPSILON, LEVEL_RAMP_MS,
    MASTER_BPM_MAX, MASTER_BPM_MIN, SongState, Tab, all_specs, decode_song_code, smoothstep,
    spec_by_id,
};

/// Requested bars per morph leg; each section rounds to complete phrases.
pub(crate) const DEFAULT_AUTO_BARS: u32 = 64;

/// Requested bars for a live toggle's first leg, rounded to whole phrases.
/// Later legs use the loop's configured request; long phrases can extend either.
const LIVE_FIRST_LEG_BARS: u32 = 16;

/// Ordered share codes for the built-in auto-morph states. The morph loops
/// through them in order, one per leg, so the list length is the number of legs
/// in a cycle.
///
/// # Adding a morph target
/// 1. In a live session, dial in the full sound you want as a destination.
/// 2. Press `Ctrl+S` to copy its `n1_…` song code to the clipboard.
/// 3. Run `just add-morph '<code>'` from the repository root. It appends the
///    state after checking the code and commits only this file.
///
/// No other code changes are needed: `decode_auto_states` picks up every entry
/// and the morph scheduler scales to any count. The future TOML mixtape
/// loader will replace this array with the same `Vec<SongState>` it decodes
/// into, so keep entries as plain share codes.
///
/// # Retiring a control these codes use
/// A code cannot carry a control this build no longer registers, so retiring
/// one means every state holding it has to be rewritten. Rewrite the values
/// deliberately; do not re-save the states from a session that already dropped
/// them, which is silent data loss. Adding the Filter module retired
/// `perc.filter` and `bass.cutoff` that way once, and the states played bare
/// noise and an open bass for a release before anyone noticed. Recovering a
/// retired control means carrying all three of these across, from the
/// pre-retirement codes in git:
///
/// - Its value. `bass.cutoff` transferred as-is — both it and the module's
///   cutoff row are Hz on a Log2 dial. `perc.filter` was
///   not a frequency: perc ran white noise through a one-pole lowpass with
///   coefficient `a = 10^(4f-4)`, whose power gain on white noise is
///   `a / (2 - a)`. The module's lowpass is two-pole, so each state took the
///   cutoff passing that same noise energy — roughly double the one-pole's
///   -3 dB corner. Matching the corner instead would have left every state
///   about 3 dB quiet, which is the whole complaint that surfaced the loss.
///   `clap.filter` was the same one-pole, but its state advanced once per
///   overlapping burst, so no formula described it. Each state took the
///   cutoff and mix minimising the squared difference from its own rendered
///   clap on the retiring build (same seed, so the same noise), which keeps
///   the brightness and lands within about half a decibel. A state at the
///   0.7 default carries that fit too (3170 Hz, 90% wet) rather than relying
///   on the factory Clap Filter, which starts fully wet.
/// - Its place in the chain. The recovered value belongs in the slot already
///   holding that module, not in the next free one: writing `bass.cutoff`
///   into slot 2 overwrote the factory Bass Drive in every state, trading one
///   silent loss for another.
/// - Its modulation. An LFO routed at a retired control is dropped with it.
///   Re-point each route at the replacement row and convert its depth, which
///   is a fraction of the dial's throw in *position* space: read the old
///   value at ±depth along the old taper, map both through the value
///   conversion above, and take the half-span between them on the new taper.
///   `kick.filter` is the easy case — its 0..1 dial position transferred
///   straight onto the Filter's then 80..8000 Hz Log2 throw as
///   `hz = 80 * 100^f`, which *is* that taper's position mapping, so its
///   depths carried over unchanged.
///
/// Widening a live dial's range is the same problem without a retirement:
/// stored values keep their units, but every route depth on the dial is a
/// fraction of a longer throw. The Filter cutoff going from 80..8000 to
/// 20..20000 Hz multiplied each cutoff LFO's depth by `ln 100 / ln 1000`
/// (2/3), which keeps its sweep the same number of octaves. Every code here
/// carries the dial-range epoch (`range_epoch.rs`), since an epoch-0 code
/// sweeping a moved dial no longer decodes; a later range change appends a
/// `RANGE_CHANGES` entry and re-authors these the same way.
///
/// Inserting the replacement module renumbers the slots after it, and a route
/// addresses a slot by number. Every LFO on a displaced module has to move
/// with it: the Filter going into Kick slot 1 pushed Drive to slot 2, so the
/// routes reading `kick.slot1.amount` became `kick.slot2.amount` or they
/// would have started modulating the new Filter's wet/dry mix instead.
///
/// Codes are re-encoded through the current build, so a slot's inert fields
/// (a one-knob module's cutoff, say) must be cleared rather than carried as
/// junk from whatever preset the slot held before.
const AUTO_STATES: &[&str] = &[
    // Baseline seed. Near-default state the cycle opens from.
    "n1_Tk9PSQIFAgAAAAIAABMAAAADAJUAAeYDxQACSjD7RPQAAGbm",
    // Sparse pad-led breakdown.
    "n1_Tk9PSQIFAgAAAAIAAHUAAAAYAAAAAI_CAQAAVFMCAABIYQMAAwIEAAMCBQADBAYAAwgIAAC4ngkAAAAACgAAmZkOAAMDEAADBRMAAwIYAAMDGgADBR0AAwMeAAMCfAAAepQzAADMTDUAAHE5NgACAACIQJUAApRCqkTFAAJKMPtEcQAARUUBOwAAAAMACgAAAMA_pDAAAABAP9XNKX4zAAAAAEFnJgAAAAAAmaB_xJUAAACAQVwPAAAAgD4AAAAAAAAAAAAAAjIAAAAACC0AAAAyAAAANwAAADAAAAA0AAAAOQAAADIAAAA3AAAADp-SwX3KueYAAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAOsAAAAuAAAAAAAAAQAACTsCAADrRAMAAwEEAAMCBQADAgYAAwgIAAAAAAkAAAAACgAA1yMQAAMFEwADARQAAwJ8AAAULjMAAFwPNQAAlByVAAJXsuFEOwECj8L1PJYAAwSXAADXI5gAAsL1aD9AAQKuR-E9OQAAcD08AADYbz4AAwJDAAD2KEUAAOhuRgAAWwnFAALWbxhDTgAAAABQAAMGUgADAVcAAFI43AAAAABaAAB7FPQAADLz9QAB7gRjAAB7FGcAAgAAAD4MAQCZGQ4BAwgPAQD__xABAhXUsUSVAgAAAEUCA_VGAgIAAJBAAYEAAAAGAAAAAACAPsxMBwAAAACy2to0CI8CZcb_f_9__7__f_-__3__fwAAAAAAQpkZBAAAAACz2to0NQAAAABAHwUAAAAAAMIYy342AAAAAECuBwAAAAAAviMbWZUAAAAAQZkZAAAAgEAAAAAAUwAAAABA6xEGAAAAAGdyy2MAAAAAAAACQgAAAAEMLQAAADQAAAA5AAAAPAAAADkAAAA0AAAAMgAAADAAAAAyAAAANwAAADQAAAAtAAAARLmlSq2DEyYAAAAAAAAAAA",
    // Full-band build with driving kick/clap and busy arp.
    "n1_Tk9PSQIFAgAAAAIAANQAAAArAAAAAAAAAQAAVFMCAABIYQQAAwEFAAMEBgADCAgAAAnXEAADAxoAA_weAAMDMwAA4Xo1AAB9JpMAAwKUAACjcJUAAwCWAAMIlwAA__-YAAIE1n1FOQAAMzM7AACqKjwAABOJPQADAa0AAv4_nEavAADMTEMAAMxMRQAAS0JGAABZb0oAAP_nxQACrktbRMcAAOtRVwAAAABaAADXI_QAAGbmYwAArkdkAAAAUGUAAFRDZgADAGcAAgAAgD4OAQMCDwEAKVw8AgAAAEQCAJlZRQID_QFdAAAABQBlAAAAAEGPAgQAAAAAlikraQ8BAAAAQc0MBAAAAAB2l4ppNQAAAABAzQwGAAAAAMIYy34zAAAAgD8AAAMAAEA_maB_xJgAAABAQR8FAAAAgD8AAAAAAAAAAAAAAjIAAAAACC0AAAAyAAAANwAAADAAAAA0AAAAOQAAADIAAAA3AAAA1971-0X8CdsAAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAIAAAAAbAAAAAOvRAQAAVFMCAABIYQQAAwIFAAMEBgADCAgAAIVrCQAA61EKAABmZhAAAwAVAAMFGgADBHwAADOzlQAB5gPFAAJKMPtETQAAexRPAAAUclAAAwNTAAMBVAADBFcAAAAA3AAAHoXeAAMC3wAAPQr0AABm5nEAAEhIdwADBQEqAAAAAgBPAAAAgEFcDwAAAAAA_60XCE0AAACAQCkcAgAAwD83UFCgAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAANEAAAAqAAAAAOvRAQAAVFMCAABIYQQAAwIFAAMEBgADCAgAAIVrCQAA61EKAABmZhAAAwAVAAMFGgADBHwAADOzMwAAFC41AADSGzYAAgAAAD6VAAHVAUMAAB8FSAACAAAAP8UAAvnt0ERNAAA9Ck8AABRyUAADA1MAAwFUAAMEVwAAAADcAAAehd4AAwLfAAA9CloAALgeXAAAC11gAAD5mGIAAFI49AAAUfj1AAGUAmMAAFI4ZAAAcSZlAAD_PWcAAgAAgD4MAQBcj3EAAEhIdwADBQFuAAAABgBlAAAAAEEfBQAAAAAAlikraUMAAAAAQM0MAgAAAADzMjuHNQAAAEBBjwIAAAAAAMIYy36VAAAAQEHjDQAAAAAAAAAAAE8AAACAQVwPAAAAAAD_rRcITQAAAIBAKRwCAADAPzdQUKAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAANUAAAAtAAAAAEfhAQAACTsCAADrRAMAAwIEAAMCBgADCAgAAACADgADARAAAwEUAAMBGAADARoAAwIdAAMBHgADAR8AAwUiAAMBJAADAicAAwEpAAMELAADAS4AAwYxAAMCMgADAXwAAACAMwAAFC41AAB9FjYAAgAAAD-VAAGACEgAAgAAgD7FAAL57dBEWgAAMzNcAAALPV4AAwD0AADW4_UAAWcOYwAArkdlAACqSGsAAwIMAQAJ13EAAFRUcgAAM7N2AABmhjwCALgeRgICAAAgQEgCA30BTAAAAAQAZQAAAIBAPQoAAAAAQJYpK2lnAAAAAEDXIwYAAAAAGL6w-0MAAADAPwoXBAAAAADzMjuHNQAAAIBBjwIAAACAQMIYy34AAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAALoAAAAnAAAAAB6FAQAAAAACAAAAAAQAAwEGAAMICwAD_hAAAwAVAAP-GgADAB8AAwMiAAMBJAADBCcAAwEpAAMALQADAi4AAwAxAAMBMgADAjMAAHA9NQAAfRY2AAMBNwACAAAAP5UAAYAIQwAAmRlFAAD1TEYAAAQaRwADAkgAAgAAgD5LAADNDMUAAsiQJUXHAACPQk4AAAAATwAAqipSAAMBUwACAACAPlQAAwNXAAAAAPQAAGbmcQAAhIQBOwAAAAIASAAAAAA_jDEHAAAAACDQBM4HAAD-__7__v_-__7_eJufiE8AAACAQY8CAAAAAAD_rRcIAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAABUBAAA5AAAAAD2KAQAAAAACAAAAAAQAAwEGAAMICwAD_hAAAwAVAAP-GgADAB8AAwMiAAMBJAADBCcAAwEpAAMALQADAi4AAwAyAAMCMwAAXA81AAAnETYAAwE3AAIAAAA_lQAB5gM5AABRuDsAAA8DPAAA2J89AAMBQAADAq8AAI9CQwAAcD1FAACgZ0YAAAQqRwADAkgAAgAAgD5LAAAAAMUAAkGLkETHAABwPU0AAHA9TgAAACBPAACqKlIAAwFTAAIAAIA-VAADA1cAAAAA3AAAPQpaAAC4HlwAALZnYAAAqlpiAAAAAPQAABTu9QABBglkAADHC2UAAP5tZgADAmcAAgAAwD9xAACEhEUCA_NGAgIAADBAAW4AAAAFAEAAAACAQOE6BAAAAAAsyOlZXAAAAIBBmRkAAAAAANyQ2CJIAAAAAD-MMQcAAAAAINAEzgcAAP7__v_-__7__v94m5-ITwAAAIBBjwIAAAAAAP-tFwhNAAAAgEHrEQAAAIBAN1BQoAAAAAAAAAJCAAAAAQwtAAAANAAAADkAAAA8AAAAOQAAADQAAAAyAAAAMAAAADIAAAA3AAAANAAAAC0AAACucsDVt5EjPAAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAACABAAA7AAAAAD2KAQAAAAACAAAAAAQAAwEGAAMICwAD_hAAAwAVAAP-GgADAB8AAwMiAAMBJAADBCcAAwEpAAMALQADAi4AAwAyAAMCMwAArcc1AAAnETYAAwE3AAIAAAA_lQACnhu3RDkAAACAOwAADwM8AAAtpT0AAwFAAAMCrwAAj0JDAABwPUUAAKBnRgAABCpHAAMCSAACAACAPksAAAAAxQACQYuQRMcAAHA9TQAArkdOAAAAIE8AAKoqUgADAVMAAgAAgD5UAAMDVwAAAADcAAA9CloAANcjXAAAtmdgAACqWmIAAAAA9AAAFO71AAEGCWQAAMcLZQAAU2NmAAMCZwACAADAP2sAAwIMAQD2KHEAAISERQID80YCAgAAMEABkAAAAAcAQAAAAIBA4ToEAAAAACzI6VlcAAAAgEGZGQAAAAAA3JDYIkgAAAAAP4wxBwAAAAAg0ATOBwAA_v_-__7__v_-_3ibn4jFAAAAAEAAAAAAAABAbfxfLgAAAAAAPwAAAAAAAACy2to0TwAAAIBBjwIAAAAAAP-tFwhNAAAAgEHrEQAAAIBAN1BQoAAAAAAAAAJCAAAAAQwtAAAANAAAADkAAAA8AAAAOQAAADQAAAAyAAAAMAAAADIAAAA3AAAANAAAAC0AAAA-b-aocDhwjgAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAEEBAABCAAAAAK5HAQAAwScCAABpLgMAAwIFAAMEBgADCAgAAAAACQAAAAAKAAAAAA8AAwEQAAMFEwADARUAAwQYAAMBGgADBx0AAwF8AAAAADMAAHA9NQAAfRY2AAIAAAA_kwADApQAAD0KlQADAJYAAwiXAAD__5gAAYUBPAAAg3o9AAMBQAADA60AASADrwAA61FDAACZGUUAAKBHRgAArjRHAAMCSAADAkoAAP_XSwAAZibFAAL0ZjZExwAACldNAADXI04AAAAATwAAFEJSAAMBUwACAADAP1QAAwNXAAAAANwAADMzXAAAYVL0AADC9fUAAWQDZAAAHBFlAAD_TWYAAwdnAAIAAIA-agADAWsAAwIMAQBSOHEAACSkdgAAmZk8AgC4HkQCAMxMRQID9UYCAgAAIEBIAgGQAEkCAgAAYEABRgAAAAMAAAAAAIA_KRwCAAAAALLa2jQ1AAAAAEKPAgAAAAAAwhjLfjYAAACAPwCABwAAAAC-IxtZBGYm_5__f_-f_58AAAAAAAACQgAAAAEMMgAAADkAAABDAAAAMgAAADIAAAAwAAAAMgAAAD4AAAAyAAAAOQAAADIAAAA0AAAAeIwdDPPogs4GAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAEcBAABDAAAAAK5HAQAAwScCAABpLgMAAwIFAAMEBgADCAgAAAAACQAAAAAKAAAAAA8AAwEQAAMFEwADARUAAwQYAAMBGgADBx0AAwF8AAAAADMAAHA9NQAAfRY2AAIAAAA_kwADApQAAD0KlQADAJYAAwiXAAD__5gAAYUBPAAAg3o9AAMBQAADA60AASADrwAA61FDAACZGUUAAKBHRgAArjRHAAMCSAADAkoAAP_XSwAAZibFAAL0ZjZExwAACldNAADXI04AAAAATwAAFEJSAAMBUwACAADAP1QAAwNXAAAAANwAADMzWgAAmRlcAABgcvQAAML19QABdwNjAADXI2QAABwRZQAAqjhnAAIAAAA-agADAWsAAwIMAQBcj3EAACSkdgAAmZk8AgC4HkQCAMxMRQID9UYCAgAAIEBIAgGQAEkCAgAAYEABjAAAAAYAZQAAAABCHwUAAAAAAJYpK2lnAAAAAEBKSQcAAAAAGL6w-wgzM_9_G8gbyBvIk6YbyP7__v9cAAAAAEKZGQAAAAAA3JDYIgAAAACAPykcAgAAAACy2to0NQAAAABCjwIAAAAAAMIYy342AAAAgD8AgAcAAAAAviMbWQRmJv-f_3__n_-fAAAAAAAAAkIAAAABDDIAAAA5AAAAQwAAADIAAAAyAAAAMAAAADIAAAA-AAAAMgAAADkAAAAyAAAANAAAAHiMHQzz6ILOBgAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAABUBAAA4AAAAAAAAAQAAAAACAAAAAAYAAwcIAAAAAAkAAAAACgAAAAB8AAAAADMAABQuNQAAfRaTAAMClAAAPQqVAAMAlgADCJcAAP__mAABNQI5AABwPTwAAIN6PQADAUAAAwOtAAEgA68AAOtRQwAAexRFAAD1TEYAAARKRwADAkoAAP_HSwAAZibFAAJg04NExwAACldOAAAAAE8AAGk3UgADAlQAAwNXAAAAANwAADMzWgAAuB5cAABhUvQAAML19QABFwRjAAB7FGQAABwRZQAAqlhmAAMHZwACAACAPmoAAwFrAAMCDAEAUjhxAAAtrXYAAJmZPAIAuB5EAgDMTEUCA_VGAgIAACBASAIBkABJAgIAAGBAAT0AAAACAAAAAAAAP-G6BwAAAACy2to0CI8C_3_-__9__v__f_9__7_-_zUAAAAAQo8CAAAAAADCGMt-AAAAAAAAAjIAAAACCDkAAAA8AAAAQAAAAD4AAAA8AAAAOQAAADQAAAA3AAAAl3Mb084gfRMAAAAAAAAAAA",
    // Sparse 145 BPM pad/tonal piece with a Wood kick and no bass, the
    // airy counterpart to 13's bass-led take on the same tempo.
    "n1_Tk9PSQIFAgAAAAIAAF0AAAASAAAAAFG4AQAAAAACAAAAAAYAAwaVAAHmA0MAAJkZRwADAkgAAgAAgD7FAALIkCVFxwAArkdNAADMTE4AAAAATwAAaVdSAAMCVwAAAADcAACFa_QAAGbmcQAALa0BcAAAAAUASAAAAAA_jDEHAAAAACDQBM4IAADLnP7_RIxTrf7__v_LnOzexQAAAABApw0AAAAAQG38Xy4AAAAAAD_rUQAAAAAAstraNE8AAAAAQs0MAAAAgED_rRcITQAAAIA-wjUAAAAAADdQUKAAAAAAAAACMgAAAAIIOQAAADwAAABAAAAAPgAAADwAAAA5AAAANAAAADcAAACXcxvTziB9EwAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAAcBAAA2AAAAAAAAAQAAAAACAAAAAAQAAwIFAAMEBgADCAgAAAAACQAAAAAKAAAAABAAAwMaAAP8HgADA3wAAAAANQAAKDE2AAIAAAA_kwADApQAAD0KlQADAJYAAwiXAAD__5gAATUCOQAAexQ7AAAAADwAAGiOPQADAUEAAwCtAAHnCa8AAFI4QwAAmRlFAABLQkYAAFlvSAADAkoAAP_fSwAAzQzFAAJg04NExwAArkdNAAAfBVcAAAAA3AAAepTeAAMC3wAAHwVaAABSOGIAAK5H9AAAo_D1AAHnBWMAAEdhZAAAAFBlAABVI2YAAwBnAAIAAIA-DAEA1yMOAQMCDwEA69FxAADBQAGBAAAABgAAAAAAgD7__wcAAAAAstraNAiPAv9__v__f_7__3__f_7__381AAAAQEG6AwAAAAAAwhjLfjMAAACAP74sAwAAQD-ZoH_EmAAAAIBAEgUAAACAPwAAAABPAAAAgEEtBAAAAAAA_60XCE0AAACAQKcHAgAAwD83UFCgAAAAAAAAAjIAAAAACC0AAAAyAAAANwAAADAAAAA0AAAAOQAAADIAAAA3AAAA25YbaIf_R_YAAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAAwBAAA3AAAAAOF6AQAAVFMCAAAEhQMAAwIEAAMCBQADAgYAAwgIAABwvQkAAClcCgAAFK4QAAMCEwADARQAAwIzAADMTDUAAD4nNgACAAAAP5MAAwKUAABSOJUAAwCWAAMIlwAA__-YAAGFATsAALodPAAA2M89AAMBrQAByQNDAABcD0UAAOhuRgAAWwnFAALWbxhDxwAAXI9NAAAULk4AAFUFTwAAFEJQAAMGUgADAVcAAFwP3AAAAADeAAMC3wAAFC5aAAAzM1wAAAtd9AAAwvX1AAFqA2QAABwhZQAA_y1nAAIAAAA-agADAWsAAwIMAQAzs3EAAMRDcgAArcc8AgBwPUUCA_NGAgIAAJhAAaMAAAAHAGUAAAAAQh8FAAAAAACWKStpZwAAAIA_ACAHAAAAABi-sPsIMzP_f_9__3__f_9__3_-__7_QAAAAIBAAAAEAAAAACzI6VlcAAAAAEIKFwAAAAAA3JDYIkgAAAAAPwAABwAAAAAg0ATOBwAA_3__f_9__3__f_9__38AAAAAgD8pXAIAAAAAstraNDUAAAAAQh8FAAAAAADCGMt-AAAAAAAA",
    // 16 with the kick driven up to half and the tonal line brought
    // forward: the same 75 BPM skeleton, pushed.
    "n1_Tk9PSQIFAgAAAAIAABkBAAA6AAAAAMJ1AQAAVFMCAAAEhQMAAwIEAAMCBQADAgYAAwgIAABwvQkAAClcCgAAFK4QAAMCEwADARQAAwIzAADMTDUAAD4nNgACAAAAP5MAAwKUAABSOJUAAwCWAAMIlwAA__-YAAGFATsAALodPAAA2M89AAMBrQAByQNDAAAAgEUAAOhuRgAAWwnFAALWbxhDxwAAXI9NAABmZk4AAFUFTwAAFDJQAAMGUQADAVIAAwFXAACPQtwAAABA3gADAt8AAKQwWgAAFC5cAABgYl8AAwRiAADMTPQAADLz9QABfQRkAAAcIWUAAP8tZwACAAAAPmoAAwFrAAMCDAEAM7NxAADEQ3IAAK3HPAIAcD1FAgPzRgICAACYQAG0AAAACABlAAAAAEIfBQAAAAAAlikraWcAAACAPwAgBwAAAAAYvrD7CDMz_3__f_9__3__f_9__v_-_0AAAACAQAAABAAAAAAsyOlZXAAAAABCChcAAAAAANyQ2CJIAAAAAD8AAAcAAAAAINAEzgcAAP9__3__f_9__3__f_9_AAAAAIA_KVwCAAAAALLa2jQ1AAAAAEIfBQAAAAAAwhjLflMAAABAP80MBgAAAABncstjAAAAAAAAAkIAAAABDC0AAAA0AAAAOQAAADwAAAA5AAAANAAAADIAAAAwAAAAMgAAADcAAAA0AAAALQAAACv0vuTCFzk0AAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAALAAAAAjAAAAAKPwAQAAVFMCAABIYQQAAwIGAAMGCQAArkcKAACjcHwAAI_CMwAAFC42AAIAAIhAlQABDAFDAAC4HkUAAPVMRgAABCpHAAMBSAACAAAAP8UAAqwzg0VNAACZGU4AAOFETwAAFHJRAAMBUgADAVMAAwFUAAMIVQACAAAAP1cAAAAA3AAAwnVaAADXI_QAADLz9QAB7gRxAABFRXIAAK3HPAIAexRFAgP0RgIDAwE7AAAAAwBDAAAAgD_rEQQAAIA-8zI7hzMAAABAQQoXAAAAAACZoH_ETwAAAABCzQwAAAAAAP-tFwgAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAADABAAA_AAAAAFG4AQAACTsCAADrRAQAAwEFAAMGBgADCAgAAI_CEAADABoAAwAfAAMCIgADASQAAwMnAAMCfAAACdczAACFazUAANJLlQAB5gM5AAAzMzsAALk9PQADAT4AAgAAQD9AAAMBrQABLg2vAADhekMAAFwPRgAAWR9HAAMDSAACAACAPkoAAP_nSwAAMzPHAACFa00AAFwPTgAA4SRPAAAUYlAAAwZRAAMCUgADBVMAAgAAgD5UAAIAAMA_VQADAVcAAAAA3AAA___eAAMC3wAAmRldAAIAAAA_XwADBGAAAE6uYgAAwnX0AACE6_UAAfUIYwAAUjhlAACqSGYAAwdnAAIAAAA-awADAg4BAwIPAQA9CjwCAPYoRAIAhWtFAgPxRgIDA0gCAzxJAgMFAeAAAAALAGcAAACAQUIIBwAAAAAYvrD7AgAA_v__fzwAAABAQc0MAAAAAAAucrS2WgAAAAA_o3AHAAAAAGfTPzkEAABlxmWGMpP_n0sAAAAAQMI1AAAAAAAVE4UiQwAAAIA-KVwHAAAAAPMyO4cEAACYuTKTzIz_n0UAAACAQQoXAAAAAAADvYdvxwAAAABAuB4AAAAAADnPJvAAAAAAgD_WowIAAAAAstraNDUAAAAgQj0KAgAAAADCGMt-MwAAACBCHkUCAAAAAJmgf8RPAAAAAEEfBQMAAAAA_60XCAAAAAAAAAJSAAAABRBAAAAAMAAAAC0AAAA0AAAAOQAAADQAAABDAAAALQAAAEAAAAA8AAAAPAAAAC0AAAAyAAAALQAAADwAAABDAAAAzfV3B-csA7sWAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAG0AAAAXAAAAAFG4AQAACTsCAADrRAQAAwEFAAMGBgADCAgAAI_CEAADABoAAwAfAAMCIgADASQAAwMnAAMCfAAACdeVAAHmA0UAAEoyRgAAriRHAAMBSAACAACAPkoAAP_HSwAAMzPHAADrUfQAAGbmAVcAAAAEAEsAAAAAQMI1AAAAAAAVE4UiQwAAAIA-PUoHAAAAAPMyO4cEAACYuTKTzIz_n8cAAAAAQLgeAAAAAAA5zybwAAAAAIA_1qMCAAAAALLa2jQAAAAAAAACMgAAAAAILQAAADIAAAA3AAAAMAAAADQAAAA5AAAAMgAAADcAAADa8rvfPi9FpgAAAAAAAAAA",
    "n1_Tk9PSQIFAgAAAAIAAOoAAAAtAAAAAFyPAQAACRsCAABBGgMAAwEEAAMBBQADBAYAAwZ8AAAAAH4AAwN_AACZGTMAAFwPNQAA0muVAALalYxFlgADApcAAMI1OQAA61E8AACDej0AAwE-AAIAAIA-sQADArIAAM0MQwAAcD1NAADMTE8AAL8sUQADAVIAAwRTAAIAAEA_VAADBlcAAAAA3AAAKRzeAAMG3wAAR2HgAAIAAIA-hgECAABAP4gBAgAAQD_sAQLMzEw-WgAA1yNcAAALXfUAAr2RK0X2AAME9wAA6xH4AALrUTg_oAECZmbmPnEAAJ0cPAIAAAABkAAAAAgAPAAAAIA_HwUFAAAAAC5ytLY5AAAAgD8KFwIAAAAAAAAAALIAAAAAQD0KBQAAAAB5NcoMAAAAAIA_AIACAAAAAAAAAAA1AAAAgEF7FAIAAAAAwhjLfjcAAACAQAAAAAAAAAAAAAAAlwAAAIA_AAAAAAAAAD26WwOXAAAAgEGZGQIAAAAAAAAAAAAAAAAAAAIyAAAABAg5AAAAMAAAADwAAAAwAAAALQAAAEAAAAA8AAAANwAAADbbHVFBpTskAQAAAAAAAAA",
    // add-morph inserts new entries immediately above this line.
];

/// Decode every `AUTO_STATES` code into a `SongState`. Fatal on the first bad
/// code: a malformed baked-in constant is a bug, not a user-facing error.
pub(crate) fn decode_auto_states() -> Vec<SongState> {
    AUTO_STATES
        .iter()
        .map(|code| {
            decode_song_code(code).unwrap_or_else(|err| {
                panic!("built-in auto-morph state {code:?} failed to decode: {err:?}")
            })
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct AutoStartError {
    number: usize,
}

impl std::fmt::Display for AutoStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "there is no song {}; --from must be in 1..={}",
            self.number,
            AUTO_STATES.len()
        )
    }
}

impl std::error::Error for AutoStartError {}

/// Full built-in order, rotated to a one-based song number without renumbering.
pub(crate) fn auto_song_numbers(from: usize) -> Result<Vec<usize>, AutoStartError> {
    if !(1..=AUTO_STATES.len()).contains(&from) {
        return Err(AutoStartError { number: from });
    }
    Ok((from..=AUTO_STATES.len()).chain(1..from).collect())
}

/// Throttle granularity for the morph writer: one 1/8 note, i.e. half a beat.
const MORPH_TICK_BEATS: f64 = 0.5;

// ============================================================
// Morph model
//
// Every leg is HOLD then TRANSITION: the `from` state is held steady for the
// requested `HOLD_FRACTION`, rounded to whole outgoing phrases; the
// crossing also rounds to whole outgoing phrases. Harmony changes at landing.
//
// During the transition each control moves by its own behavior, derived from
// its `ControlKind`:
//
//   Tempo — ease toward the destination or its nearby half/double-time
//       bridge; unmatched large gaps wait for a jump at the landing.
//
//   Glide (Gain/Continuous) — lerp `from`→`to` across the transition window.
//       Levels glide too, except the drum cuts below. Perc and Clap exit on
//       the transition downbeat when their target levels are zero. Kick is silent
//       for every percentage transition and returns on the next song downbeat.
//
//   Swing — any slot holding Swing on either endpoint waits through the full
//       crossing. Its identity and Amount change together on the next song's
//       downbeat, whether Swing is arriving, leaving, or changing Amount.
//
//   Moving module — if the same module moves to another slot on its layer,
//       both slots and their automation wait for the next song downbeat. This
//       preserves one live instance throughout the crossing.
//
//   Module swap — a slot whose module (or delay clock) differs between the two
//       states changes its identity and rows together on the
//       transition downbeat. Its rows mean different things under different
//       modules, so blending them (a Reverb's size dragged toward a Filter's
//       Hz) is never a valid in-between. Swing and moving-module slots wait.
//
//   Snap (Discrete/Timing)  — never interpolated; hold `from`, then hard-jump.
//       Harmony (progression, chord window/length, custom chords and voicing)
//       waits until the next song downbeat. Arp pattern snaps at the crossing.
//       Other grid params stagger in at 8-bar offsets after it, in
//       registry order, so similar sections hard-switch rather than crossfade.
// ============================================================

/// Requested hold share before both sections round to outgoing phrases.
const HOLD_FRACTION: f64 = 2.0 / 3.0;

/// Spacing between successive non-structural grid hard-switches, in bars.
const STAGGER_STEP_BARS: f64 = 8.0;

/// Harmony belongs to the outgoing song until the next song lands. This
/// includes custom chord definitions and voicing, not only window selection.
fn waits_for_harmony(spec_id: &str) -> bool {
    spec_id == "pad.progression" || spec_id.starts_with("pad.chord")
}

/// Drum levels that cut on exit instead of gliding to zero.
const EXIT_DRUM_TABS: [Tab; 2] = [Tab::Perc, Tab::Clap];

fn is_drum_level(spec_id: &str) -> bool {
    EXIT_DRUM_TABS
        .iter()
        .any(|tab| tab.level_id() == Some(spec_id))
}

fn snaps_drum_level(spec_id: &str, from: f32, to: f32) -> bool {
    is_drum_level(spec_id) && from > 0.0 && to == 0.0
}

/// Every performing voice's level/gain row: each non-Master tab's
/// `Tab::level_id`, so a new voice joins the energy metrics by being a tab.
fn voice_level_specs() -> impl Iterator<Item = &'static ControlSpec> {
    Tab::all()
        .into_iter()
        .filter(|tab| *tab != Tab::Master)
        .filter_map(Tab::level_id)
        .filter_map(spec_by_id)
}

/// Crude "how different do two states sound" metric: the summed absolute
/// difference of every performing element's level/gain. Deliberately simple —
/// used only to pick which built-in state a live auto-toggle heads toward first.
fn level_distance(a: &FluidControls, b: &FluidControls) -> f32 {
    voice_level_specs()
        .map(|spec| ((spec.get)(a) - (spec.get)(b)).abs())
        .sum()
}

/// How one control crosses a leg. `move_of` is the only place that decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    /// Follow the leg's tempo plan, including half/double-time landings.
    Tempo,
    /// Lerp across the transition window.
    Glide,
    /// Hold `from` through the hold, then go silent for the crossing.
    Drop,
    /// Hold `from` for the full leg; the next leg begins at `to`.
    Wait,
    /// Hold `from`, jump to `to` on the transition downbeat.
    Snap,
    /// Hold `from`, jump to `to` at this row's staggered offset after it.
    Stagger,
}

/// The single rule table for morph behavior, first match wins. Everything
/// that must not blend lives here; the default follows the row's `ControlKind`.
fn move_of(
    spec: &ControlSpec,
    from: &FluidControls,
    to: &FluidControls,
    swapped: &[(&str, usize)],
    held: &[(&str, usize)],
) -> Move {
    if spec.id == "master.bpm" {
        return Move::Tempo;
    }
    if Tab::Kick.level_id() == Some(spec.id) {
        return Move::Drop;
    }
    if waits_for_harmony(spec.id) || in_slot_set(held, spec.id) {
        return Move::Wait;
    }
    if spec.id == "arp.pattern"
        || in_slot_set(swapped, spec.id)
        || snaps_drum_level(spec.id, (spec.get)(from), (spec.get)(to))
    {
        return Move::Snap;
    }
    match spec.kind {
        ControlKind::Gain | ControlKind::Continuous => Move::Glide,
        ControlKind::Timing | ControlKind::Discrete => Move::Stagger,
    }
}

/// (spec index into `all_specs()` order, jump offset in bars from the
/// transition downbeat) for every changed non-structural grid param on a leg,
/// staggered in registry order. Structural and glide params aren't listed.
fn stepped_offsets(
    from: &FluidControls,
    to: &FluidControls,
    held: &[(&str, usize)],
) -> Vec<(usize, f64)> {
    let swapped = swapped_slots(from, to);
    all_specs()
        .enumerate()
        .filter(|(_, spec)| move_of(spec, from, to, &swapped, held) == Move::Stagger)
        .filter(|(_, spec)| (spec.get)(from) != (spec.get)(to))
        .enumerate()
        .map(|(order, (index, _))| (index, (order + 1) as f64 * STAGGER_STEP_BARS))
        .collect()
}

fn in_slot_set(slots: &[(&str, usize)], spec_id: &str) -> bool {
    parse_module_slot_id(spec_id).is_some_and(|(layer, slot, _)| slots.contains(&(layer, slot)))
}

/// Hold a slot until the next song if it carries Swing or a module that moves
/// to another slot on the same layer. A moving Filter must never be doubled by
/// loading its target slot early, or lost by clearing its source slot early.
fn held_slots(from: &FluidControls, to: &FluidControls) -> Vec<(&'static str, usize)> {
    let identities: Vec<_> = all_specs()
        .filter_map(|spec| {
            let (layer, slot, field) = parse_module_slot_id(spec.id)?;
            (field == ModuleSlotField::Kind).then_some((
                layer,
                slot,
                (spec.get)(from),
                (spec.get)(to),
            ))
        })
        .collect();
    let mut held = Vec::new();
    for &(layer, slot, from_kind, to_kind) in &identities {
        if [from_kind, to_kind]
            .into_iter()
            .any(|value| module_kind_at(value).is_some_and(|kind| kind.id == "swing"))
        {
            held.push((layer, slot));
        }
    }
    for &(layer, source, kind, _) in &identities {
        if module_kind_at(kind).is_none()
            || identities
                .iter()
                .filter(|&&(candidate_layer, _, candidate_kind, _)| {
                    candidate_layer == layer && candidate_kind == kind
                })
                .count()
                != 1
        {
            continue;
        }
        let mut targets = identities
            .iter()
            .filter(|&&(candidate_layer, _, _, target_kind)| {
                candidate_layer == layer && target_kind == kind
            });
        let (Some(&(_, target, _, _)), None) = (targets.next(), targets.next()) else {
            continue;
        };
        if source != target {
            for slot in [source, target] {
                if !held.contains(&(layer, slot)) {
                    held.push((layer, slot));
                }
            }
        }
    }
    held
}

/// The module slots (layer prefix, 0-based slot) whose module or clock differs
/// between two states. Their rows mean different things on each side, so a leg
/// snaps their identity and rows instead of blending them, except when Swing
/// occupies either endpoint or a module moves between slots; those wait.
fn swapped_slots(from: &FluidControls, to: &FluidControls) -> Vec<(&'static str, usize)> {
    let mut swapped = Vec::new();
    for spec in all_specs() {
        let Some((layer, slot, field)) = parse_module_slot_id(spec.id) else {
            continue;
        };
        let identity = matches!(
            field,
            ModuleSlotField::Kind | ModuleSlotField::Clock | ModuleSlotField::RightClock
        );
        if identity && (spec.get)(from) != (spec.get)(to) && !swapped.contains(&(layer, slot)) {
            swapped.push((layer, slot));
        }
    }
    swapped
}

/// Where the morph is right now: the state actually sounding, the one it will
/// become, and how far across it is — `None` while the leg is still holding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MorphPosition {
    pub(crate) playing: Option<usize>,
    pub(crate) next: Option<usize>,
    pub(crate) blend: Option<f32>,
}

/// Config for the slow-evolution morph between song states, published to the
/// audio thread via `ArcSwap<Option<MorphState>>` alongside controls and
/// automation. Live progress is derived from the beat clock, not stored, so
/// it stays deterministic for any future offline render.
#[derive(Clone)]
pub(crate) struct MorphState {
    endpoints: Vec<SongState>,
    morph_ids: Vec<Option<usize>>,
    /// Cumulative leg starts, including the end of one complete cycle.
    leg_starts: Vec<f64>,
    timings: Vec<LegTiming>,
    tempo_moves: Vec<TempoMove>,
    /// Staggered hard-switch offsets for leg i -> i+1 (mod n), precomputed once.
    stepped: Vec<Vec<(usize, f64)>>,
    /// Slots held as a unit through leg i -> i+1 because they carry Swing or
    /// a module moving between slot addresses.
    held: Vec<Vec<(&'static str, usize)>>,
    /// Module slots that swap whole on the transition downbeat for leg i ->
    /// i+1 (mod n), precomputed once.
    swapped: Vec<Vec<(&'static str, usize)>>,
    /// Engine beat the morph timeline is anchored to. Zero for the baked-in
    /// loop (which starts at beat 0); set to the toggle beat for a live start
    /// so the first leg begins from the current state, not mid-loop.
    origin_beat: f64,
    /// A live toggle has a shorter first leg and preserves the sounding phrase.
    first_leg: Option<LegTiming>,
}

/// A 15 BPM gap at 80 BPM, expressed as a faster/slower ratio so the
/// threshold is symmetric in both directions. Shared by glides and bridges.
const TEMPO_MATCH_RATIO: f32 = 1.0 + 15.0 / 80.0;

#[derive(Clone, Copy, Debug)]
enum TempoMove {
    /// Glide to this tempo during the crossing; the next leg restores its
    /// authored BPM, giving an exact half/double-time switch when applicable.
    Glide(f32),
    /// Keep the outgoing tempo until the destination downbeat.
    Jump,
}

impl TempoMove {
    fn new(from: f32, to: f32) -> Self {
        let distance = |bpm: f32| from.max(bpm) / from.min(bpm);
        if distance(to) <= TEMPO_MATCH_RATIO {
            return Self::Glide(to);
        }
        let bridge = [to * 0.5, to * 2.0]
            .into_iter()
            .filter(|bpm| (MASTER_BPM_MIN..=MASTER_BPM_MAX).contains(bpm))
            .filter(|bpm| distance(*bpm) <= TEMPO_MATCH_RATIO)
            .min_by(|a, b| distance(*a).total_cmp(&distance(*b)));
        bridge.map_or(Self::Jump, Self::Glide)
    }

    fn bpm_at(self, from: f32, progress: f32) -> f32 {
        match self {
            Self::Glide(to) => from + (to - from) * smoothstep(progress),
            Self::Jump => from,
        }
    }
}

/// Hold and crossing each span whole phrases of the outgoing song.
#[derive(Clone, Copy)]
struct LegTiming {
    hold: f64,
    crossing: f64,
}

impl LegTiming {
    fn new(from: &FluidControls, bars: u32) -> Self {
        let requested = f64::from(bars.max(1)) * 4.0;
        let round_phrases = |beats: f64, controls: &FluidControls| {
            let phrase = super::ChordWindow::requested(&controls.pad).phrase_beats(&controls.pad);
            (beats / phrase).round().max(1.0) * phrase
        };
        Self {
            hold: round_phrases(requested * HOLD_FRACTION, from),
            crossing: round_phrases(requested * (1.0 - HOLD_FRACTION), from),
        }
    }

    fn beats(self) -> f64 {
        self.hold + self.crossing
    }

    fn progress_at(self, elapsed: f64) -> f64 {
        // Use the trigger grid's tolerance so its downbeat cannot fire while
        // the morph still considers the same sample part of the crossing.
        let elapsed = if (elapsed - self.hold).abs() <= GRID_BEAT_EPSILON {
            self.hold
        } else {
            elapsed.max(0.0)
        };
        elapsed / self.beats()
    }
}

impl MorphState {
    pub(crate) fn new(endpoints: Vec<SongState>, bars: u32) -> Self {
        assert!(
            !endpoints.is_empty(),
            "auto-morph requires at least one state"
        );
        let n = endpoints.len();
        let held: Vec<_> = (0..n)
            .map(|i| held_slots(&endpoints[i].controls, &endpoints[(i + 1) % n].controls))
            .collect();
        let stepped = (0..n)
            .map(|i| {
                stepped_offsets(
                    &endpoints[i].controls,
                    &endpoints[(i + 1) % n].controls,
                    &held[i],
                )
            })
            .collect();
        let swapped = (0..n)
            .map(|i| swapped_slots(&endpoints[i].controls, &endpoints[(i + 1) % n].controls))
            .collect();
        let timings: Vec<_> = (0..n)
            .map(|i| LegTiming::new(&endpoints[i].controls, bars))
            .collect();
        let tempo_moves = (0..n)
            .map(|i| {
                TempoMove::new(
                    endpoints[i].controls.master.bpm,
                    endpoints[(i + 1) % n].controls.master.bpm,
                )
            })
            .collect();
        let mut leg_starts = vec![0.0];
        for timing in &timings {
            leg_starts.push(leg_starts.last().copied().unwrap_or(0.0) + timing.beats());
        }
        Self {
            endpoints,
            morph_ids: (1..=n).map(Some).collect(),
            leg_starts,
            timings,
            tempo_moves,
            stepped,
            held,
            swapped,
            origin_beat: 0.0,
            first_leg: None,
        }
    }

    /// A morph over a hand-picked set of songs, each carrying the number it
    /// should report rather than its position in this cycle — so `nooise 9,12`
    /// still reads `song 9 → 12` instead of `1 → 2`. One song is a legal
    /// cycle: its controls stay fixed except for the crossing's Kick drop.
    pub(crate) fn labelled(endpoints: Vec<SongState>, labels: Vec<usize>, bars: u32) -> Self {
        assert_eq!(
            endpoints.len(),
            labels.len(),
            "every morph endpoint needs its label"
        );
        let mut morph = Self::new(endpoints, bars);
        morph.morph_ids = labels.into_iter().map(Some).collect();
        morph
    }

    /// Build a morph for a live toggle at `start_beat`: endpoint 0 is the
    /// caller's current controls and automation (LFO/envelope routes),
    /// so nothing changes instantly — the morph just starts moving from where
    /// it already is, taking whatever modulators are live along for the ride
    /// instead of leaving them running unmodified forever. It heads to the
    /// *nearest* built-in state first (by `level_distance`), then loops the
    /// rest.
    pub(crate) fn from_live(
        current: FluidControls,
        current_automation: AutomationState,
        states: Vec<SongState>,
        bars: u32,
        start_beat: f64,
    ) -> Self {
        let nearest = states
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                level_distance(&current, &a.controls)
                    .total_cmp(&level_distance(&current, &b.controls))
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut endpoints = Vec::with_capacity(states.len() + 1);
        endpoints.push(SongState {
            automation: current_automation,
            ..SongState::from_controls(current)
        });
        endpoints.extend(states[nearest..].iter().cloned());
        endpoints.extend(states[..nearest].iter().cloned());
        let mut morph = Self::new(endpoints, bars);
        morph.morph_ids = std::iter::once(None)
            .chain((nearest + 1..=states.len()).map(Some))
            .chain((1..=nearest).map(Some))
            .collect();
        morph.origin_beat = start_beat.max(0.0);
        morph.first_leg = Some(LegTiming::new(
            &morph.endpoints[0].controls,
            LIVE_FIRST_LEG_BARS,
        ));
        // The caller can replace this with the actual sounding phrase anchor.
        morph.align_live_phrase(0.0);
        morph
    }

    fn leg_timing(&self, leg_index: i64) -> LegTiming {
        if leg_index == 0
            && let Some(first) = self.first_leg
        {
            return first;
        }
        self.timings[leg_index.rem_euclid(self.endpoints.len() as i64) as usize]
    }

    fn leg_beats(&self, leg_index: i64) -> f64 {
        self.leg_timing(leg_index).beats()
    }

    fn leg_transition_start_beat(&self, leg_index: i64) -> f64 {
        self.leg_timing(leg_index).hold
    }

    /// Preserve the live cursor's phase instead of treating the toggle as beat one.
    fn align_live_phrase(&mut self, phrase_start: f64) {
        if let Some(first) = &mut self.first_leg {
            let controls = &self.endpoints[0].controls.pad;
            let phrase = super::ChordWindow::requested(controls).phrase_beats(controls);
            let requested_hold =
                LegTiming::new(&self.endpoints[0].controls, LIVE_FIRST_LEG_BARS).hold;
            let desired = self.origin_beat + requested_hold;
            let boundary = phrase_start + ((desired - phrase_start) / phrase).round() * phrase;
            first.hold = (boundary - self.origin_beat).max(
                phrase_start
                    + ((self.origin_beat - phrase_start) / phrase).floor() * phrase
                    + phrase
                    - self.origin_beat,
            );
        }
    }

    pub(crate) fn restarted(&self) -> Self {
        let mut restarted = self.clone();
        restarted.origin_beat = 0.0;
        if restarted.first_leg.is_some() {
            restarted.first_leg = Some(LegTiming::new(
                &restarted.endpoints[0].controls,
                LIVE_FIRST_LEG_BARS,
            ));
        }
        for state in &mut restarted.endpoints {
            state.automation.restart();
        }
        restarted
    }

    /// Locate a leg in the precomputed variable-length cycle without walking
    /// elapsed loops. Only the first live leg can override that cycle.
    fn leg_at_indexed(&self, beat: f64) -> (usize, usize, f64, i64) {
        let elapsed = (beat - self.origin_beat).max(0.0);
        let first_end = self.origin_beat + self.leg_beats(0);
        if beat + GRID_BEAT_EPSILON < first_end {
            return (
                0,
                1 % self.endpoints.len(),
                self.leg_timing(0).progress_at(elapsed),
                0,
            );
        }
        // Subtract the absolute landing once: subtracting a fractional live
        // origin and first-leg duration separately can miss later boundaries.
        let cycle_beat = beat - first_end + self.timings[0].beats();
        let cycle_beats = self.leg_starts[self.endpoints.len()];
        let cycle = ((cycle_beat + GRID_BEAT_EPSILON) / cycle_beats).floor() as i64;
        let within_cycle = (cycle_beat - cycle as f64 * cycle_beats).max(0.0);
        let from = self
            .leg_starts
            .partition_point(|start| *start <= within_cycle + GRID_BEAT_EPSILON)
            - 1;
        let leg_index = cycle * self.endpoints.len() as i64 + from as i64;
        let t = self.timings[from].progress_at(within_cycle - self.leg_starts[from]);
        (from, (from + 1) % self.endpoints.len(), t, leg_index)
    }

    /// Anchor harmony at each song landing. The outgoing phrase continues
    /// through the crossing; a live first leg retains its sounding cursor.
    pub(crate) fn phrase_start_at(&self, beat: f64) -> Option<f64> {
        let (from, _, _, leg_index) = self.leg_at_indexed(beat);
        if leg_index == 0 {
            return self.first_leg.is_none().then_some(self.origin_beat);
        }
        let cycle = leg_index / self.endpoints.len() as i64;
        Some(
            self.origin_beat + self.leg_beats(0) - self.timings[0].beats()
                + cycle as f64 * self.leg_starts[self.endpoints.len()]
                + self.leg_starts[from],
        )
    }

    fn section_at(&self, beat: f64) -> (i64, bool) {
        let (_, _, t, leg) = self.leg_at_indexed(beat);
        (
            leg,
            t * self.leg_beats(leg) >= self.leg_transition_start_beat(leg),
        )
    }

    /// (from index, to index, t in [0,1)) for the leg containing `beat`.
    #[cfg(test)]
    fn leg_at(&self, beat: f64) -> (usize, usize, f64) {
        let (from, to, t, _) = self.leg_at_indexed(beat);
        (from, to, t)
    }

    /// What is sounding at `beat`. A leg holds its `from` state for whole
    /// outgoing phrases before crossing, so through that hold the honest
    /// answer is one state playing, not a morph in progress — reporting the
    /// pair the whole time names a transition that has not started.
    pub(crate) fn position_at(&self, beat: f64) -> MorphPosition {
        let (from, to, t, leg_index) = self.leg_at_indexed(beat);
        let beats_per_leg = self.leg_beats(leg_index);
        let transition_start = self.leg_transition_start_beat(leg_index);
        let t_beat = t * beats_per_leg;
        let blend = (t_beat >= transition_start).then(|| {
            let span = (beats_per_leg - transition_start).max(1e-6);
            (((t_beat - transition_start) / span) as f32).clamp(0.0, 1.0)
        });
        MorphPosition {
            playing: self.morph_ids[from],
            next: self.morph_ids[to],
            blend,
        }
    }

    /// One tempo curve for the published controls and the audio-rate clock.
    /// At landing, leg selection returns the incoming song's authored BPM.
    pub(crate) fn tempo_at(&self, beat: f64) -> f32 {
        let (from, _, t, leg) = self.leg_at_indexed(beat);
        let timing = self.leg_timing(leg);
        let progress =
            ((t * timing.beats() - timing.hold) / timing.crossing).clamp(0.0, 1.0) as f32;
        self.tempo_moves[from].bpm_at(self.endpoints[from].controls.master.bpm, progress)
    }

    /// Fade the Kick layer's existing voice and effect tail during the last
    /// 30 ms of the hold. The whole percentage phase is exactly silent, and
    /// the next leg opens at full gain for the destination's first hit.
    pub(crate) fn kick_gain_at(&self, beat: f64, bpm: f64) -> f32 {
        let (_, _, t, leg_index) = self.leg_at_indexed(beat);
        let remaining_beats =
            self.leg_transition_start_beat(leg_index) - t * self.leg_beats(leg_index);
        let fade_beats = f64::from(LEVEL_RAMP_MS) * 0.001 * bpm.max(1.0) / 60.0;
        smoothstep((remaining_beats / fade_beats).clamp(0.0, 1.0) as f32)
    }

    /// The morphed `FluidControls` at `beat`: hold `from`, then glide or
    /// hard-switch each control across the transition window (see the module
    /// comment). At the leg boundary `leg_at` wraps to the next leg's `from`,
    /// which equals this leg's `to`, so the real target lands exactly on "one".
    pub(crate) fn controls_at(&self, beat: f64) -> FluidControls {
        let (from_idx, to_idx, t, leg_index) = self.leg_at_indexed(beat);
        let from = &self.endpoints[from_idx].controls;
        let to = &self.endpoints[to_idx].controls;
        let offsets = &self.stepped[from_idx];
        let swapped = &self.swapped[from_idx];
        let held = &self.held[from_idx];
        let beats_per_leg = self.leg_beats(leg_index);
        let t_beat = t * beats_per_leg;
        let transition_start = self.leg_transition_start_beat(leg_index);
        let transition_beats = (beats_per_leg - transition_start).max(1e-6);

        let mut next = from.clone();
        for (index, spec) in all_specs().enumerate() {
            let from_v = (spec.get)(from);
            let to_v = (spec.get)(to);

            let value = match move_of(spec, from, to, swapped, held) {
                Move::Tempo => self.tempo_at(beat),
                Move::Drop => {
                    if t_beat < transition_start {
                        from_v
                    } else {
                        0.0
                    }
                }
                Move::Wait => from_v,
                Move::Snap => {
                    if t_beat < transition_start {
                        from_v
                    } else {
                        to_v
                    }
                }
                Move::Glide => {
                    let tt =
                        ((t_beat - transition_start) / transition_beats).clamp(0.0, 1.0) as f32;
                    from_v + (to_v - from_v) * tt
                }
                Move::Stagger => {
                    let jump_beat = match offsets.iter().find(|(i, _)| *i == index) {
                        Some(&(_, offset_bars)) => {
                            (transition_start + offset_bars * 4.0).min(beats_per_leg)
                        }
                        None => transition_start, // unchanged: `from` and `to` are identical
                    };
                    spec.quantize(if t_beat < jump_beat { from_v } else { to_v })
                }
            };

            (spec.set)(&mut next, value);
        }
        next
    }

    /// The morphed `AutomationState` at `beat`: the `AutomationState`
    /// counterpart to `controls_at`. Every LFO/envelope route snaps its
    /// non-level fields together at the single transition downbeat (no
    /// per-field staggering, unlike `controls_at`'s grid params) while its
    /// level field glides continuously — see `AutomationState::morph` for the
    /// full rationale. Harmony and held module-slot automation stay at their
    /// source state until the next song, matching the controls they modulate.
    pub(crate) fn automation_at(&self, beat: f64) -> AutomationState {
        let (from_idx, to_idx, t, leg_index) = self.leg_at_indexed(beat);
        let from = &self.endpoints[from_idx].automation;
        let to = &self.endpoints[to_idx].automation;
        let beats_per_leg = self.leg_beats(leg_index);
        let t_beat = t * beats_per_leg;
        let transition_start = self.leg_transition_start_beat(leg_index);
        let transition_beats = (beats_per_leg - transition_start).max(1e-6);
        let tt = ((t_beat - transition_start) / transition_beats).clamp(0.0, 1.0) as f32;
        let mut automation = AutomationState::morph(from, to, tt, t_beat >= transition_start);
        automation.hold_addresses_from(from, |address| {
            waits_for_harmony(address.id()) || in_slot_set(&self.held[from_idx], address.id())
        });
        automation
    }
}

/// A morph-less `ArcSwap`, shared by every entry point that doesn't run
/// `nooise auto`.
pub(crate) fn no_morph() -> Arc<ArcSwap<Option<MorphState>>> {
    Arc::new(ArcSwap::from_pointee(None))
}

/// Everything the UI thread needs to drive auto mode: the shared morph handle
/// (also held by the audio engine) plus the built-in states and leg length to
/// spin up a fresh morph on demand. Owns the on/off mechanics so the UI never
/// touches the `ArcSwap` directly.
pub(crate) struct AutoControls {
    morph: Arc<ArcSwap<Option<MorphState>>>,
    states: Vec<SongState>,
    bars: u32,
}

impl AutoControls {
    pub(crate) fn new(
        morph: Arc<ArcSwap<Option<MorphState>>>,
        states: Vec<SongState>,
        bars: u32,
    ) -> Self {
        Self {
            morph,
            states,
            bars,
        }
    }

    /// True while a morph is running (auto mode is on).
    pub(crate) fn is_running(&self) -> bool {
        self.morph.load().is_some()
    }

    pub(crate) fn position_at(&self, beat: f64) -> Option<MorphPosition> {
        self.morph
            .load_full()
            .as_ref()
            .as_ref()
            .map(|morph| morph.position_at(beat))
    }

    /// Leave auto mode. The engine stops rewriting controls and automation,
    /// so the current morphed values stay live and editable. A no-op when
    /// already off.
    pub(crate) fn exit(&self) {
        self.morph.store(Arc::new(None));
    }

    /// Flip auto mode. Turning on builds a morph anchored at `beat` starting
    /// from `current`/`current_automation`, so nothing jumps — any live
    /// LFO/envelope routes ride along with the morph instead of being
    /// left behind; turning off just calls `exit`.
    pub(crate) fn toggle(
        &self,
        current: FluidControls,
        current_automation: AutomationState,
        beat: f64,
        phrase_start: f64,
    ) {
        if self.is_running() {
            self.exit();
        } else {
            let mut state = MorphState::from_live(
                current,
                current_automation,
                self.states.clone(),
                self.bars,
                beat,
            );
            state.align_live_phrase(phrase_start);
            self.morph.store(Arc::new(Some(state)));
        }
    }
}

/// The engine publishes regular morph updates on absolute eighth-note ticks.
/// Section boundaries bypass throttling so structure and Kick arrive together.
#[derive(Default)]
pub(crate) struct MorphWriter {
    last_tick: Option<i64>,
    last_section: Option<(i64, bool)>,
}

impl MorphWriter {
    pub(crate) fn boundary_due(&self, morph: &MorphState, beat: f64) -> bool {
        self.last_section != Some(morph.section_at(beat))
    }

    /// `Some((controls, automation))` when a new morph tick is due at `beat`;
    /// `None` otherwise (call site should skip the write).
    pub(crate) fn tick(
        &mut self,
        morph: &MorphState,
        beat: f64,
    ) -> Option<(FluidControls, AutomationState)> {
        let tick = (beat / MORPH_TICK_BEATS).floor() as i64;
        if self.last_tick == Some(tick) && !self.boundary_due(morph, beat) {
            return None;
        }
        self.last_tick = Some(tick);
        self.last_section = Some(morph.section_at(beat));
        Some((morph.controls_at(beat), morph.automation_at(beat)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::module::preset_slot;
    use super::*;

    #[test]
    fn auto_from_last_song_wraps_and_keeps_original_labels() {
        let states = decode_auto_states();
        let count = states.len();
        let numbers = auto_song_numbers(count).unwrap();
        assert_eq!(numbers.len(), count);
        assert_eq!(numbers[0], count);
        assert_eq!(numbers[1], 1);
        let selected = numbers
            .iter()
            .map(|number| states[number - 1].clone())
            .collect();
        let morph = MorphState::labelled(selected, numbers, 4);
        let mut beat = 0.0;
        for leg in 0..=(count * 2) {
            let expected = if leg % count == 0 { count } else { leg % count };
            assert_eq!(morph.position_at(beat).playing, Some(expected));
            assert_eq!(
                morph.controls_at(beat).master.bpm,
                states[expected - 1].controls.master.bpm
            );
            beat += morph.leg_beats(leg as i64);
        }
        assert_eq!(
            auto_song_numbers(1).unwrap(),
            (1..=count).collect::<Vec<_>>()
        );
        for invalid in [0, count + 1, usize::MAX] {
            assert!(auto_song_numbers(invalid).is_err());
        }
    }

    #[test]
    fn tempo_chooses_glide_half_double_or_landing_jump() {
        for (source, target, bridge) in [
            (100.0, 115.0, 115.0),
            (115.0, 100.0, 100.0),
            (80.0, 150.0, 75.0),
            (150.0, 80.0, 160.0),
            (70.0, 140.0, 70.0),
            (140.0, 70.0, 140.0),
            (80.0, 120.0, 80.0),
            (120.0, 80.0, 120.0),
            (80.0, 95.0, 95.0),
            (80.0, 96.0, 80.0),
            (80.0, 190.0, 95.0),
            (80.0, 191.0, 80.0),
            (80.0, 135.0, 67.5),
            (135.0, 80.0, 160.0),
            (30.0, 200.0, 30.0),
        ] {
            let mut from = phrase_controls();
            from.master.bpm = source;
            let mut to = from.clone();
            to.master.bpm = target;
            let morph = MorphState::new(
                vec![SongState::from_controls(from), SongState::from_controls(to)],
                6,
            );
            assert_eq!(morph.controls_at(15.0).master.bpm, source);
            assert_eq!(morph.controls_at(16.0).master.bpm, source);
            assert!(
                (morph.controls_at(20.0).master.bpm - (source + bridge) * 0.5).abs() < 0.001,
                "{source} -> {target}: midpoint should head toward {bridge}"
            );
            assert!((morph.controls_at(23.999).master.bpm - bridge).abs() < 0.001);
            assert_eq!(morph.controls_at(24.0).master.bpm, target);
        }
    }

    #[test]
    fn songs_three_and_four_cross_on_complete_phrases_in_both_directions() {
        let states = decode_auto_states();
        for (from, to) in [(2, 3), (3, 2)] {
            let morph = MorphState::new(vec![states[from].clone(), states[to].clone()], 4);
            let source = &states[from].controls.pad;
            let source_phrase = f64::from(source.chord_bars * 4.0 * source.chord_count);
            let hold = morph.leg_transition_start_beat(0);
            let crossing = morph.leg_beats(0) - hold;
            assert!(
                hold >= source_phrase && hold % source_phrase == 0.0,
                "{} -> {}: hold {hold} splits source phrase {source_phrase}",
                from + 1,
                to + 1
            );
            assert!(
                crossing >= source_phrase && crossing % source_phrase == 0.0,
                "{} -> {}: crossing {crossing} splits source phrase {source_phrase}",
                from + 1,
                to + 1
            );
        }
    }

    #[test]
    fn custom_harmony_and_its_automation_wait_until_landing() {
        let mut from = phrase_controls();
        from.pad.progression = crate::fluid::voice::CUSTOM_PROGRESSION_INDEX as f32;
        let mut to = from.clone();
        to.pad.chord_slots[0].degree = 5.0;
        to.pad.chord_slots[0].inversion = 2.0;
        to.pad.chord_count = 3.0;
        to.pad.chord_bars = 0.5;
        let address = ControlAddress::new("pad.chord1_degree");
        let mut target = SongState::from_controls(to.clone());
        target.automation.set_route(address, LfoRoute::default());
        let morph = MorphState::new(vec![SongState::from_controls(from.clone()), target], 6);
        for beat in [16.0, 20.0, 23.999] {
            assert_eq!(
                morph.controls_at(beat).pad.chord_slots[0].degree,
                from.pad.chord_slots[0].degree
            );
            assert_eq!(
                morph.controls_at(beat).pad.chord_slots[0].inversion,
                from.pad.chord_slots[0].inversion
            );
            assert_eq!(morph.controls_at(beat).pad.chord_bars, from.pad.chord_bars);
            assert!(morph.automation_at(beat).route(address).is_none());
        }
        assert_eq!(
            morph.controls_at(24.0).pad.chord_slots[0].degree,
            to.pad.chord_slots[0].degree
        );
        assert!(morph.automation_at(24.0).route(address).is_some());
    }

    #[test]
    fn writer_does_not_drift_past_a_musical_boundary() {
        let morph = MorphState::new(vec![SongState::default()], 4);
        let mut writer = MorphWriter::default();
        assert!(writer.tick(&morph, 0.01).is_some());
        assert!(
            writer.tick(&morph, 0.501).is_some(),
            "the half-beat boundary must not wait until 0.51"
        );
    }

    #[test]
    fn live_toggle_preserves_the_sounding_phrase_and_repeats_variable_legs() {
        let mut source = phrase_controls();
        source.pad.chord_count = 3.0;
        source.pad.chord_bars = 0.5;
        let mut target = source.clone();
        target.pad.chord_count = 2.0;
        let morph_handle = no_morph();
        let auto = AutoControls::new(
            Arc::clone(&morph_handle),
            vec![SongState::from_controls(target)],
            4,
        );
        // A previous chord edit started a six-beat phrase at beat 2.
        // Toggling partway through it must preserve that anchor.
        auto.toggle(source, AutomationState::default(), 3.37, 2.0);
        let loaded = morph_handle.load();
        let morph = loaded.as_ref().as_ref().unwrap();
        assert_eq!(morph.position_at(43.99).blend, None);
        assert_eq!(morph.phrase_start_at(43.99), None);
        assert_eq!(morph.position_at(44.0).blend, Some(0.0));
        assert_eq!(morph.phrase_start_at(44.0), None);
        assert_eq!(morph.position_at(68.0).playing, Some(1));
        assert_eq!(morph.position_at(68.0).blend, None);
        // Only the first leg uses the 16-bar request. Later legs use four:
        // target -> source = 12 + 4 beats; source -> target = 12 + 6.
        assert_eq!(morph.position_at(84.0).playing, None);
        assert_eq!(morph.position_at(102.0).playing, Some(1));
        assert_eq!(morph.position_at(118.0).playing, None);
    }

    #[test]
    fn a_filter_moving_between_slots_stays_single_and_keeps_its_automation() {
        let states = decode_auto_states();
        for (from, to, source_slot, source_id, target_id) in [
            (2, 3, 0, "perc.slot1.time", "perc.slot2.time"),
            (3, 2, 1, "perc.slot2.time", "perc.slot1.time"),
        ] {
            let morph = MorphState::new(vec![states[from].clone(), states[to].clone()], 64);
            let source = ControlAddress::new(source_id);
            let target = ControlAddress::new(target_id);
            for beat in [168.0, 212.0, 255.5] {
                let controls = morph.controls_at(beat);
                let filters: Vec<_> = controls
                    .modules
                    .perc
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, value)| {
                        value
                            .kind()
                            .is_some_and(|kind| kind.id == "filter")
                            .then_some(slot)
                    })
                    .collect();
                assert_eq!(
                    filters,
                    [source_slot],
                    "song {} -> {} at beat {beat}",
                    from + 1,
                    to + 1
                );
                let automation = morph.automation_at(beat);
                assert_eq!(automation.lfo_lanes(source).len(), 1);
                assert_eq!(automation.lfo_lanes(target).len(), 0);
                assert_eq!(
                    automation.lfo_lanes(source)[0].depth_ratio,
                    states[from].automation.lfo_lanes(source)[0].depth_ratio
                );
            }
            let landed = morph.controls_at(256.0);
            let target_slot = 1 - source_slot;
            assert_eq!(
                landed.modules.perc[target_slot].kind().unwrap().id,
                "filter"
            );
            assert_eq!(morph.automation_at(256.0).lfo_lanes(target).len(), 1);
        }
    }

    #[test]
    fn built_in_morphs_do_not_duplicate_or_drop_a_shared_effect() {
        let states = decode_auto_states();
        for from in 0..states.len() {
            let to = (from + 1) % states.len();
            let morph = MorphState::new(vec![states[from].clone(), states[to].clone()], 64);
            for tab in Tab::all() {
                let Some(from_slots) = states[from].controls.modules.for_tab(tab) else {
                    continue;
                };
                let to_slots = states[to].controls.modules.for_tab(tab).unwrap();
                for kind in super::super::module::MODULE_CATALOG {
                    let count =
                        |slots: &[super::super::module::ModuleSlot;
                              super::super::module::MODULE_SLOTS]| {
                            slots
                                .iter()
                                .filter(|slot| {
                                    slot.kind().is_some_and(|loaded| loaded.id == kind.id)
                                })
                                .count()
                        };
                    if count(from_slots) != 1 || count(to_slots) != 1 {
                        continue;
                    }
                    for beat in [168.0, 212.0, 255.5] {
                        let controls = morph.controls_at(beat);
                        assert_eq!(
                            count(controls.modules.for_tab(tab).unwrap()),
                            1,
                            "song {} -> {}, {tab:?} {} at beat {beat}",
                            from + 1,
                            to + 1,
                            kind.id
                        );
                    }
                }
            }
        }
    }

    /// (playing, next) for a beat, the pair the tests care about.
    fn ids(morph: &MorphState, beat: f64) -> (Option<usize>, Option<usize>) {
        let at = morph.position_at(beat);
        (at.playing, at.next)
    }

    /// Two four-beat chords make an explicit eight-beat phrase for control tests.
    fn phrase_controls() -> FluidControls {
        let mut controls = FluidControls::default();
        controls.pad.chord_bars = 1.0;
        controls.pad.chord_count = 2.0;
        controls
    }

    fn state(bpm: f32) -> FluidControls {
        let mut c = phrase_controls();
        c.master.bpm = bpm;
        c
    }

    /// Sum of every performing element's level/gain: the audible-energy proxy
    /// the never-silent invariant is checked against.
    fn total_level(c: &FluidControls) -> f32 {
        voice_level_specs().map(|spec| (spec.get)(c)).sum()
    }

    #[test]
    fn auto_states_all_decode_without_error() {
        let states = decode_auto_states();
        assert_eq!(states.len(), AUTO_STATES.len());
    }

    #[test]
    fn built_in_perc_levels_and_sweeps_keep_their_balance_after_the_output_trim() {
        let previous_levels = [
            0.0, 0.10, 0.02, 0.16, 0.0, 0.06, 0.06, 0.08, 0.02, 0.26, 0.08, 0.08, 0.06, 0.0, 0.0,
            0.10, 0.10, 0.06, 0.14, 0.0, 0.02,
        ];
        let previous_depths = [
            (1, 0.050003815),
            (3, 0.0),
            (14, 0.058258947),
            (17, 0.029999238),
            (18, 0.08999771),
        ];
        let states = decode_auto_states();
        let address = ControlAddress::new("perc.level");
        for (index, (state, previous)) in states.iter().zip(previous_levels).enumerate() {
            let lanes = state.automation.lfo_lanes(address);
            let expected_depth = previous_depths.iter().find(|(song, _)| *song == index);
            assert_eq!(lanes.len(), usize::from(expected_depth.is_some()));
            assert!(state.automation.envelope_lanes(address).is_empty());
            assert!(
                (state.controls.perc.level / 3.0 - previous).abs() < 0.00001,
                "song {} changed its percussion balance",
                index + 1
            );
            if let Some((_, depth)) = expected_depth {
                assert!(
                    (lanes[0].depth_ratio / 3.0 - depth).abs() < 0.00001,
                    "song {} changed its percussion sweep",
                    index + 1
                );
                assert!(state.controls.perc.level + lanes[0].depth_ratio < 1.0);
            }
        }
    }

    /// Every built-in cutoff LFO keeps its authored sweep: these lows and
    /// highs are measured through the engine's own modulation sum. Song indexes
    /// 1 and 3 were re-authored on the 20..20000 Hz dial and are pinned there; the
    /// rest were measured on the last build with the 80..8000 Hz dial
    /// (`a54b594`), where a lane the widening did not rescale would open
    /// about 1.5x wider in octaves.
    #[test]
    fn built_in_cutoff_sweeps_cover_the_hz_they_did_before_the_dial_widened() {
        const SWEEPS: [(usize, &str, f32, f32); 6] = [
            (1, "perc.slot1.time", 899.9, 2061.6),
            (2, "perc.slot1.time", 904.9, 3602.5),
            (3, "perc.slot2.time", 3537.2, 4663.2),
            (5, "perc.slot1.time", 322.4, 682.2),
            (13, "kick.slot1.time", 1832.7, 3829.1),
            (14, "perc.slot2.time", 492.8, 647.8),
        ];
        let states = decode_auto_states();
        for (index, id, low, high) in SWEEPS {
            let song = &states[index];
            let address = ControlAddress::new(id);
            let spec = address.spec().contextual(&song.controls);
            let lanes = song.automation.lfo_lanes(address);
            let (swept_low, swept_high) = (0..6_400)
                .map(|step| {
                    modulated_control_value_full(
                        &spec,
                        lanes,
                        &[],
                        (spec.get)(&song.controls),
                        ModContext::lfo_only(f64::from(step) / 100.0),
                    )
                })
                .fold((f32::MAX, f32::MIN), |(lo, hi), hz| {
                    (lo.min(hz), hi.max(hz))
                });
            for (label, measured, expected) in [("low", swept_low, low), ("high", swept_high, high)]
            {
                assert!(
                    (measured / expected - 1.0).abs() < 1e-3,
                    "song {index} {id} {label}: {measured} Hz, was {expected} Hz"
                );
            }
        }
    }

    /// Perc is white noise: unfiltered it is both brighter and louder than
    /// anything these states were dialed against. The voice used to own a
    /// lowpass; retiring it for the Filter module once dropped that cutoff out
    /// of every built-in state and left the perc bare. Every state carries a
    /// working Filter, silent ones included — a leg glides the cutoff between
    /// its endpoints, so a state parked at 8 kHz makes the approach to a percy
    /// one open up rather than hold the darkness it was dialed with.
    #[test]
    fn every_perc_chain_still_filters_its_noise() {
        for (index, state) in decode_auto_states().iter().enumerate() {
            let c = &state.controls;
            let slot = super::super::module::chain_amount_slot(&c.modules.perc, "filter")
                .unwrap_or_else(|| panic!("auto state {} has no perc Filter", index + 1));
            let filter = &c.modules.perc[slot];
            assert!(
                filter.amount > 0.0 && filter.time < 8_000.0,
                "auto state {} has an inert perc Filter ({} Hz at {})",
                index + 1,
                filter.time,
                filter.amount
            );
        }
    }

    /// Bass ships with a Drive in its chain and several states raise it. A
    /// recovered `bass.cutoff` was once written into that slot instead of the
    /// Filter beside it, which silently deleted the Drive from all of them.
    #[test]
    fn every_bass_chain_keeps_its_drive() {
        for (index, state) in decode_auto_states().iter().enumerate() {
            let c = &state.controls;
            let slot = super::super::module::chain_amount_slot(&c.modules.bass, "drive")
                .unwrap_or_else(|| panic!("auto state {} lost its bass Drive", index + 1));
            assert!(
                c.modules.bass[slot].amount > 0.0,
                "auto state {} has an inert bass Drive",
                index + 1
            );
        }
    }

    #[test]
    fn leg_math_two_states_wraps_forever() {
        let morph = MorphState::new(
            vec![
                SongState::from_controls(state(80.0)),
                SongState::from_controls(state(120.0)),
            ],
            2,
        );
        // A short request still gets one eight-beat hold and one crossing phrase.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(8.0), (0, 1, 0.5));
        assert_eq!(morph.leg_at(16.0), (1, 0, 0.0));
        assert_eq!(morph.leg_at(24.0), (1, 0, 0.5));
        assert_eq!(morph.leg_at(32.0), (0, 1, 0.0));
    }

    #[test]
    fn leg_math_eight_states_wraps_forever() {
        let endpoints: Vec<SongState> = (0..8)
            .map(|i| SongState::from_controls(state(80.0 + i as f32)))
            .collect();
        let morph = MorphState::new(endpoints, 1);
        // Every short leg lasts two eight-beat phrases.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(112.0), (7, 0, 0.0));
        assert_eq!(morph.leg_at(120.0), (7, 0, 0.5));
        assert_eq!(morph.leg_at(128.0), (0, 1, 0.0));
    }

    /// The footer reads this. Through the hold there is one song sounding and
    /// no transition to report; once the leg crosses, both ends and the
    /// progress between them are real.
    /// A hand-picked set reports the numbers that were asked for, not the
    /// positions they landed in, and one song is a cycle that simply holds.
    #[test]
    fn a_chosen_set_keeps_the_song_numbers_it_was_given() {
        let endpoints: Vec<SongState> = [90.0, 120.0]
            .into_iter()
            .map(|bpm| SongState::from_controls(state(bpm)))
            .collect();
        let morph = MorphState::labelled(endpoints, vec![9, 12], 1);
        assert_eq!(ids(&morph, 0.0), (Some(9), Some(12)));
        assert_eq!(ids(&morph, 16.0), (Some(12), Some(9)));

        let held = MorphState::labelled(vec![SongState::from_controls(state(99.0))], vec![9], 1);
        assert_eq!(ids(&held, 0.0), (Some(9), Some(9)));
        assert_eq!(held.controls_at(0.0).master.bpm, 99.0);
        assert_eq!(held.controls_at(400.0).master.bpm, 99.0);
    }

    #[test]
    fn a_leg_reports_one_song_until_it_actually_crosses() {
        let endpoints: Vec<SongState> = (0..2)
            .map(|i| SongState::from_controls(state(80.0 + i as f32)))
            .collect();
        // 6 bars/leg -> holds through beat 15.9, crosses from 16 to 24.
        let morph = MorphState::new(endpoints, 6);

        let held = morph.position_at(8.0);
        assert_eq!((held.playing, held.next), (Some(1), Some(2)));
        assert_eq!(held.blend, None, "the hold is not a morph in progress");

        let crossing = morph.position_at(20.0);
        assert_eq!((crossing.playing, crossing.next), (Some(1), Some(2)));
        assert!((crossing.blend.expect("crossing") - 0.5).abs() < 1e-3);

        // The next leg's hold reports the state it landed on.
        let landed = morph.position_at(24.0);
        assert_eq!(landed.playing, Some(2));
        assert_eq!(landed.blend, None);
    }

    #[test]
    fn morph_ids_match_the_one_based_auto_state_list() {
        let endpoints: Vec<SongState> = (0..3)
            .map(|i| SongState::from_controls(state(80.0 + i as f32)))
            .collect();
        let morph = MorphState::new(endpoints, 1);

        assert_eq!(ids(&morph, 16.0), (Some(2), Some(3)));
    }

    #[test]
    fn leg_math_boundaries_at_t_zero_and_towards_one() {
        let morph = MorphState::new(
            vec![
                SongState::from_controls(state(80.0)),
                SongState::from_controls(state(120.0)),
            ],
            1,
        );
        let (_, _, t_start) = morph.leg_at(0.0);
        assert_eq!(t_start, 0.0);
        let (from, to, t_end) = morph.leg_at(15.999_999);
        assert_eq!((from, to), (0, 1));
        assert!(t_end > 0.99);
    }

    #[test]
    fn gain_param_lerps_linearly() {
        // The short request rounds to one eight-beat phrase per section.
        let mut from = phrase_controls();
        from.pad.level = 0.0;
        let mut to = phrase_controls();
        to.pad.level = 1.0;
        let morph = MorphState::new(
            vec![SongState::from_controls(from), SongState::from_controls(to)],
            1,
        );
        let controls = morph.controls_at(8.0 + 8.0 * 0.25);
        assert!((controls.pad.level - 0.25).abs() < 1e-4);
    }

    #[test]
    fn drum_exits_cut_together_on_the_transition_downbeat() {
        let mut from = phrase_controls();
        from.perc.level = 0.4;
        from.kick.level = 0.8;
        from.clap.level = 0.6;
        let to = phrase_controls();
        // 6 bars/leg -> transition downbeat at beat 16.
        let morph = MorphState::new(
            vec![
                SongState::from_controls(from.clone()),
                SongState::from_controls(to),
            ],
            6,
        );

        let before = morph.controls_at(15.9);
        assert_eq!(before.perc.level, from.perc.level);
        assert_eq!(before.kick.level, from.kick.level);
        assert_eq!(before.clap.level, from.clap.level);

        let cut = morph.controls_at(16.0);
        assert_eq!(cut.perc.level, 0.0);
        assert_eq!(cut.kick.level, 0.0);
        assert_eq!(cut.clap.level, 0.0);
    }

    #[test]
    fn kick_is_absent_from_every_percentage_transition_and_returns_on_the_next_song() {
        let from = phrase_controls();
        let mut to = phrase_controls();
        to.perc.level = 0.4;
        to.kick.level = 0.8;
        to.clap.level = 0.6;
        // 6 bars/leg -> transition runs from beat 16 through beat 24.
        let morph = MorphState::new(
            vec![SongState::from_controls(from), SongState::from_controls(to)],
            6,
        );

        let mid = morph.controls_at(20.0);
        assert!((mid.perc.level - 0.2).abs() < 1e-4);
        assert_eq!(mid.kick.level, 0.0);
        assert!((mid.clap.level - 0.3).abs() < 1e-4);
        for beat in [16.0, 20.0, 23.99, 40.0, 44.0, 47.99] {
            assert!(morph.position_at(beat).blend.is_some());
            assert_eq!(morph.controls_at(beat).kick.level, 0.0, "beat {beat}");
        }
        assert_eq!(morph.controls_at(15.99).kick.level, 0.0);
        assert_eq!(morph.controls_at(24.0).kick.level, 0.8);
        assert_eq!(morph.controls_at(39.99).kick.level, 0.8);
        assert_eq!(morph.controls_at(48.0).kick.level, 0.0);
    }

    #[test]
    fn a_steady_kick_also_drops_for_the_crossing() {
        let mut song = phrase_controls();
        song.kick.level = 0.6;
        let morph = MorphState::new(vec![SongState::from_controls(song)], 6);
        assert_eq!(morph.controls_at(15.99).kick.level, 0.6);
        assert_eq!(morph.controls_at(16.0).kick.level, 0.0);
        assert_eq!(morph.controls_at(23.99).kick.level, 0.0);
        assert_eq!(morph.controls_at(24.0).kick.level, 0.6);
        assert_eq!(morph.kick_gain_at(15.9, 120.0), 1.0);
        assert!(morph.kick_gain_at(15.97, 120.0) < 1.0);
        assert_eq!(morph.kick_gain_at(16.0, 120.0), 0.0);
        assert_eq!(morph.kick_gain_at(24.0, 120.0), 1.0);
    }

    #[test]
    fn glide_holds_through_hold_window_then_lerps() {
        let mut from = phrase_controls();
        from.modules.master[0].amount = 0.0;
        let mut to = phrase_controls();
        to.modules.master[0].amount = 1.0;
        // 6 bars/leg: hold 4 bars (transition_start=16 beats), transition 8 beats.
        let morph = MorphState::new(
            vec![SongState::from_controls(from), SongState::from_controls(to)],
            6,
        );
        // Deep in the hold window: still `from`.
        assert!((morph.controls_at(8.0).modules.master[0].amount - 0.0).abs() < 1e-4);
        // Halfway through the transition (beat 20 of 24): ~0.5.
        assert!((morph.controls_at(20.0).modules.master[0].amount - 0.5).abs() < 1e-3);
        // Near the end of the transition: essentially `to`.
        assert!(morph.controls_at(23.9).modules.master[0].amount > 0.98);
    }

    /// A morph snaps every phrase control on one downbeat, so the phrase
    /// restarts once, into the destination's window and chord length.
    #[test]
    fn a_morph_restarts_the_phrase_once() {
        use crate::fluid::{ProgressionCursor, TimingContext};
        let from = phrase_controls();
        let mut to = phrase_controls();
        to.pad.chord_bars = 1.0;
        to.pad.chord_count = 4.0;
        to.pad.progression = 3.0;
        to.pad.chord_offset = 4.0;
        // 6 bars/leg -> transition downbeat at beat 16.
        let morph = MorphState::new(
            vec![SongState::from_controls(from), SongState::from_controls(to)],
            6,
        );
        let mut cursor = ProgressionCursor::new(&morph.controls_at(0.0).pad);
        let mut restarts = 0;
        let mut slots = Vec::new();
        for tick in 0..(44 * 16) {
            let beat = f64::from(tick) / 16.0;
            let before = cursor;
            if cursor.tick(
                &morph.controls_at(beat).pad,
                TimingContext {
                    morph_phrase_start: morph.phrase_start_at(beat),
                    ..TimingContext::new(48_000.0, 120.0, beat)
                },
            ) {
                restarts += usize::from(
                    cursor.window != before.window || cursor.chord_beats != before.chord_beats,
                );
                if beat >= 24.0 {
                    slots.push(cursor.slot());
                }
            }
        }
        assert_eq!(restarts, 1);
        assert_eq!(cursor.window.progression, 3);
        assert_eq!(cursor.chord_beats, 4.0);
        assert_eq!(slots[..5], [4, 5, 6, 7, 4]);
    }

    #[test]
    fn harmony_waits_for_landing_while_arp_pattern_snaps_at_crossing() {
        let from = phrase_controls();
        let mut to = phrase_controls();
        to.pad.progression = 3.0;
        to.pad.chord_count = 3.0;
        to.arp.pattern = 2.0;
        // 6 bars/leg -> transition downbeat at beat 16.
        let morph = MorphState::new(
            vec![
                SongState::from_controls(from.clone()),
                SongState::from_controls(to.clone()),
            ],
            6,
        );

        // Just before the downbeat: all three still hold `from`.
        let before = morph.controls_at(15.9);
        assert_eq!(before.pad.progression, from.pad.progression);
        assert_eq!(before.pad.chord_count, from.pad.chord_count);
        assert_eq!(before.arp.pattern, from.arp.pattern);

        let crossing = morph.controls_at(16.0);
        assert_eq!(crossing.pad.progression, from.pad.progression);
        assert_eq!(crossing.pad.chord_count, from.pad.chord_count);
        assert_eq!(crossing.arp.pattern, 2.0);

        let landed = morph.controls_at(24.0);
        assert_eq!(landed.pad.progression, 3.0);
        assert_eq!(landed.pad.chord_count, 3.0);
        assert_eq!(landed.arp.pattern, 2.0);
    }

    #[test]
    fn nonstructural_grid_param_staggers_after_the_structural_downbeat() {
        let from = phrase_controls();
        let mut to = phrase_controls();
        to.tonal.synth_type = 1.0; // one changed non-structural grid param -> 8-bar offset
        // 30 bars/leg: hold 20 bars (transition_start=80), its jump at 80+8*4=112.
        let morph = MorphState::new(
            vec![
                SongState::from_controls(from.clone()),
                SongState::from_controls(to.clone()),
            ],
            30,
        );

        // Still holds through the structural downbeat and up to its own offset.
        assert_eq!(
            morph.controls_at(80.0).tonal.synth_type,
            from.tonal.synth_type
        );
        assert_eq!(
            morph.controls_at(111.0).tonal.synth_type,
            from.tonal.synth_type
        );
        // At its 8-bar offset it hard-switches.
        assert_eq!(morph.controls_at(112.0).tonal.synth_type, 1.0);
    }

    #[test]
    fn morph_never_dips_below_the_quieter_endpoint() {
        let mut loud = phrase_controls();
        loud.pad.level = 0.9;
        loud.kick.level = 0.8;
        loud.bass.level = 0.7;
        let mut quiet = phrase_controls();
        quiet.pad.level = 0.2;
        quiet.perc.level = 0.0;
        quiet.kick.level = 0.0;
        quiet.tonal.level = 0.0;
        quiet.clap.level = 0.0;
        quiet.bass.level = 0.1;
        quiet.arp.gain = 0.0;
        let floor = total_level(&loud).min(total_level(&quiet));
        let morph = MorphState::new(
            vec![
                SongState::from_controls(loud),
                SongState::from_controls(quiet),
            ],
            8,
        );
        let beats_per_leg = 32.0;

        for i in 0..=64 {
            let beat = beats_per_leg * i as f64 / 64.0;
            assert!(
                total_level(&morph.controls_at(beat)) >= floor - 1e-4,
                "morph dipped below the quieter endpoint at beat {beat}"
            );
        }
    }

    #[test]
    fn from_live_starts_at_current_and_heads_to_nearest_state() {
        let mut current = phrase_controls();
        current.pad.level = 0.5;
        let mut far = phrase_controls();
        far.pad.level = 1.0;
        far.kick.level = 1.0;
        far.bass.level = 1.0;
        let mut near = phrase_controls();
        near.pad.level = 0.5; // matches current, everything else default -> closest

        let morph = MorphState::from_live(
            current.clone(),
            AutomationState::default(),
            vec![
                SongState::from_controls(far),
                SongState::from_controls(near.clone()),
            ],
            4,
            0.0,
        );
        // Endpoint 0 is exactly the current state: toggling on changes nothing.
        assert_eq!(morph.endpoints[0].controls.pad.level, current.pad.level);
        // First target is the nearest built-in state, not the far one.
        assert_eq!(morph.endpoints[1].controls.pad.level, near.pad.level);
        assert_eq!(morph.endpoints[1].controls.kick.level, near.kick.level);
    }

    #[test]
    fn from_live_timeline_starts_at_the_toggle_beat() {
        let mut current = phrase_controls();
        current.pad.level = 0.2;
        let mut target = phrase_controls();
        target.pad.level = 0.9;
        // Toggle on at beat 1000: the leg must start there, not mid-loop.
        let morph = MorphState::from_live(
            current.clone(),
            AutomationState::default(),
            vec![SongState::from_controls(target)],
            4,
            1000.0,
        );
        // At the toggle beat, output is exactly `current` (leg 0, t=0).
        assert!((morph.controls_at(1000.0).pad.level - 0.2).abs() < 1e-4);
    }

    #[test]
    fn live_first_leg_uses_the_short_request_before_phrase_rounding() {
        let mut current = phrase_controls();
        current.pad.level = 0.2;
        let mut target = phrase_controls();
        target.pad.level = 0.9;
        // The loop's normal leg length (64 bars) would otherwise push the
        // transition out past bar 40; a live toggle's first leg must not
        // wait that long.
        let morph = MorphState::from_live(
            current,
            AutomationState::default(),
            vec![SongState::from_controls(target)],
            DEFAULT_AUTO_BARS,
            0.0,
        );
        let sixteen_bars_beat = 16.0 * 4.0;
        assert!(
            (morph.controls_at(sixteen_bars_beat).pad.level - 0.9).abs() < 1e-3,
            "expected the transition to have completed by bar 16"
        );
        // Confirm it actually is the loop's normal 64-bar cadence past leg 0,
        // not a global shrink of every leg's length.
        assert_eq!(morph.leg_beats(1), DEFAULT_AUTO_BARS as f64 * 4.0);
    }

    #[test]
    fn writer_throttles_to_one_tick_per_eighth_note() {
        let morph = MorphState::new(
            vec![
                SongState::from_controls(state(80.0)),
                SongState::from_controls(state(120.0)),
            ],
            64,
        );
        let mut writer = MorphWriter::default();

        assert!(
            writer.tick(&morph, 0.0).is_some(),
            "first tick always fires"
        );
        assert!(
            writer.tick(&morph, 0.1).is_none(),
            "within the same 1/8 note, no new write"
        );
        assert!(
            writer.tick(&morph, 0.5).is_some(),
            "a full 1/8 note later, a new write is due"
        );
    }

    fn lfo_route(depth_ratio: f32) -> LfoRoute {
        LfoRoute {
            depth_ratio,
            ..LfoRoute::default()
        }
    }

    #[test]
    fn automation_route_present_on_both_sides_glides_depth_and_snaps_shape() {
        let mut from_state = phrase_controls();
        from_state.modules.master[0].amount = 0.0;
        let mut to_state = phrase_controls();
        to_state.modules.master[0].amount = 0.0;

        let address = ControlAddress::new("pad.level");
        let mut from_auto = AutomationState::default();
        from_auto.set_route(
            address,
            LfoRoute {
                depth_ratio: 0.2,
                shape: LfoShape::Sine,
                ..LfoRoute::default()
            },
        );
        let mut to_auto = AutomationState::default();
        to_auto.set_route(
            address,
            LfoRoute {
                depth_ratio: 0.8,
                shape: LfoShape::Square,
                ..LfoRoute::default()
            },
        );

        let morph = MorphState::new(
            vec![
                SongState {
                    automation: from_auto,
                    ..SongState::from_controls(from_state)
                },
                SongState {
                    automation: to_auto,
                    ..SongState::from_controls(to_state)
                },
            ],
            6,
        );
        // 6 bars/leg -> transition downbeat at beat 16, transition ends at beat 24.

        // Deep in the hold window: depth and shape both still `from`.
        let held = morph.automation_at(8.0);
        assert!((held.route(address).unwrap().depth_ratio - 0.2).abs() < 1e-4);
        assert_eq!(held.route(address).unwrap().shape, LfoShape::Sine);

        // Halfway through the transition: depth has glided, shape has already
        // snapped to `to` at the downbeat (not interpolated).
        let mid = morph.automation_at(20.0);
        assert!((mid.route(address).unwrap().depth_ratio - 0.5).abs() < 1e-3);
        assert_eq!(mid.route(address).unwrap().shape, LfoShape::Square);

        // End of the transition: essentially `to`.
        let done = morph.automation_at(23.9);
        assert!(done.route(address).unwrap().depth_ratio > 0.78);
    }

    #[test]
    fn automation_route_added_by_target_fades_in_from_silence() {
        let from_auto = AutomationState::default();
        let address = ControlAddress::new("pad.level");
        let mut to_auto = AutomationState::default();
        to_auto.set_route(address, lfo_route(0.6));

        let morph = MorphState::new(
            vec![
                SongState {
                    automation: from_auto,
                    ..SongState::from_controls(phrase_controls())
                },
                SongState {
                    automation: to_auto,
                    ..SongState::from_controls(phrase_controls())
                },
            ],
            6,
        );

        // Present but silent (depth 0) during the hold window — functionally
        // identical to absent, since a zero-depth route has no audible effect.
        assert!((morph.automation_at(0.0).route(address).unwrap().depth_ratio).abs() < 1e-4);
        // Fading in during the transition, silent at its start.
        let mid = morph.automation_at(20.0);
        assert!((mid.route(address).unwrap().depth_ratio - 0.3).abs() < 1e-3);
    }

    #[test]
    fn automation_route_removed_by_target_fades_out_and_does_not_reappear() {
        // Three states so leg 1 (endpoint 1 -> endpoint 2) doesn't loop
        // straight back to the routed endpoint 0.
        let address = ControlAddress::new("pad.level");
        let mut routed = AutomationState::default();
        routed.set_route(address, lfo_route(0.6));
        let unrouted = AutomationState::default();

        let morph = MorphState::new(
            vec![
                SongState {
                    automation: routed,
                    ..SongState::from_controls(phrase_controls())
                },
                SongState {
                    automation: unrouted.clone(),
                    ..SongState::from_controls(phrase_controls())
                },
                SongState {
                    automation: unrouted,
                    ..SongState::from_controls(phrase_controls())
                },
            ],
            6,
        );

        // Still full depth during the hold window of leg 0 (endpoint 0 -> 1).
        assert!((morph.automation_at(0.0).route(address).unwrap().depth_ratio - 0.6).abs() < 1e-4);
        // Fading out during the transition.
        let mid = morph.automation_at(20.0);
        assert!((mid.route(address).unwrap().depth_ratio - 0.3).abs() < 1e-3);
        // Gone once this leg's `to` (endpoint 1, unrouted) becomes leg 1's
        // `from`: beat 24 is the start of leg 1 (endpoint 1 -> 2, neither
        // routed).
        assert!(morph.automation_at(24.0).route(address).is_none());
    }

    #[test]
    fn from_live_carries_current_automation_as_endpoint_zero() {
        let address = ControlAddress::new("pad.level");
        let mut current_auto = AutomationState::default();
        current_auto.set_route(address, lfo_route(0.5));

        let morph = MorphState::from_live(
            phrase_controls(),
            current_auto.clone(),
            vec![SongState::from_controls(phrase_controls())],
            4,
            0.0,
        );
        assert_eq!(
            morph.endpoints[0]
                .automation
                .route(address)
                .unwrap()
                .depth_ratio,
            0.5
        );
    }

    /// A module's Time/Amount rows mean different things under different
    /// modules (a Reverb's size 0..1, a Filter's cutoff in Hz), so a slot whose
    /// module changes across a leg never blends its params. Gliding them while
    /// the old kind still sounds once drove a Reverb's size to 4000 and its wet
    /// mix to full on noise while the level rose ninefold: song 3 to 4.
    #[test]
    fn a_slot_whose_module_changes_never_blends_its_params() {
        let states = decode_auto_states();
        let n = states.len();
        for from in 0..n {
            let to = (from + 1) % n;
            let morph = MorphState::new(vec![states[from].clone(), states[to].clone()], 64);
            for spec in all_specs() {
                let Some((layer, slot, field)) = parse_module_slot_id(spec.id) else {
                    continue;
                };
                let kind_id = format!("{layer}.slot{}.kind", slot + 1);
                let kind = spec_by_id(&kind_id).expect("slot kind row");
                let (a, b) = (
                    (spec.get)(&states[from].controls),
                    (spec.get)(&states[to].controls),
                );
                if (kind.get)(&states[from].controls) == (kind.get)(&states[to].controls)
                    || field == ModuleSlotField::Kind
                {
                    continue;
                }
                for step in 0..=256 {
                    let v = (spec.get)(&morph.controls_at(f64::from(step)));
                    assert!(
                        v == a || v == b,
                        "song {} -> {}: {} blended to {v} between {a} and {b} at beat {step} while its module changes",
                        from + 1,
                        to + 1,
                        spec.id
                    );
                }
            }
        }
    }

    #[test]
    fn swing_amount_waits_for_the_next_song_downbeat() {
        let swung = |amount: f32| {
            let mut c = phrase_controls();
            c.modules.perc[0] = preset_slot("swing", amount);
            c
        };
        let morph = MorphState::new(
            vec![
                SongState::from_controls(swung(0.1)),
                SongState::from_controls(swung(0.6)),
            ],
            64,
        );
        let amount =
            |beat: f64| (spec_by_id("perc.slot1.amount").unwrap().get)(&morph.controls_at(beat));
        assert_eq!(amount(168.0), 0.1);
        assert_eq!(amount(212.0), 0.1);
        assert_eq!(amount(255.5), 0.1);
        assert_eq!(amount(256.0), 0.6);
    }

    #[test]
    fn outgoing_swing_waits_until_the_next_song_downbeat() {
        let mut from = phrase_controls();
        from.modules.perc[0] = preset_slot("swing", 0.6);
        let mut to = phrase_controls();
        to.modules.perc[0] = preset_slot("filter", 1.0);
        let morph = MorphState::new(
            vec![SongState::from_controls(from), SongState::from_controls(to)],
            64,
        );
        let before = morph.controls_at(255.5);
        assert_eq!(before.modules.perc[0].kind().unwrap().id, "swing");
        assert_eq!(before.modules.perc[0].amount, 0.6);
        let after = morph.controls_at(256.0);
        assert_eq!(after.modules.perc[0].kind().unwrap().id, "filter");
        assert_eq!(after.modules.perc[0].amount, 1.0);
    }

    #[test]
    fn song_three_to_four_adds_swing_on_song_fours_downbeat() {
        let states = decode_auto_states();
        let morph = MorphState::new(vec![states[2].clone(), states[3].clone()], 64);
        for (tab, slot) in [(Tab::Perc, 0), (Tab::Arp, 1)] {
            let id = match tab {
                Tab::Perc => "perc.slot1.amount",
                Tab::Arp => "arp.slot2.amount",
                _ => unreachable!(),
            };
            let target = (spec_by_id(id).unwrap().get)(&states[3].controls);
            let before = morph.controls_at(167.5);
            assert!(
                !before.modules.for_tab(tab).unwrap()[slot]
                    .kind()
                    .is_some_and(|kind| kind.id == "swing")
            );
            let mut crossing = morph.controls_at(212.0);
            assert!(
                !crossing.modules.for_tab(tab).unwrap()[slot]
                    .kind()
                    .is_some_and(|kind| kind.id == "swing")
            );
            super::super::module::resolve_module_chain(&mut crossing);
            match tab {
                Tab::Perc => assert_eq!(crossing.perc.swing, 0.0),
                Tab::Arp => assert_eq!(crossing.arp.swing, 0.0),
                _ => unreachable!(),
            }
            let almost = morph.controls_at(255.5);
            assert!(
                !almost.modules.for_tab(tab).unwrap()[slot]
                    .kind()
                    .is_some_and(|kind| kind.id == "swing")
            );
            let arrived = morph.controls_at(256.0);
            assert_eq!(
                arrived.modules.for_tab(tab).unwrap()[slot]
                    .kind()
                    .unwrap()
                    .id,
                "swing"
            );
            assert!(((spec_by_id(id).unwrap().get)(&arrived) - target).abs() < 1e-4);
        }
    }
}
