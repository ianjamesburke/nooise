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
use super::automation::{ControlAddress, LfoRoute, LfoShape};
use super::{
    AutomationState, ControlKind, ControlSpec, FluidControls, SongState, Tab, all_specs,
    decode_song_code, spec_by_id,
};

/// Bars per morph leg, matching the throttled-writer granularity of one leg
/// spanning `bars * 4` beats (4/4).
pub(crate) const DEFAULT_AUTO_BARS: u32 = 64;

/// Bars for a live toggle's first leg. Short on purpose: pressing `a` mid-vibe
/// should start audibly moving toward the destination almost immediately
/// instead of waiting out a full slow-evolution leg. Every leg after this one
/// reverts to the loop's normal `bars` length.
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
///   cutoff row are Hz over the same 80..8000 Log2 range. `perc.filter` was
///   not a frequency: perc ran white noise through a one-pole lowpass with
///   coefficient `a = 10^(4f-4)`, whose power gain on white noise is
///   `a / (2 - a)`. The module's lowpass is two-pole, so each state took the
///   cutoff passing that same noise energy — roughly double the one-pole's
///   -3 dB corner. Matching the corner instead would have left every state
///   about 3 dB quiet, which is the whole complaint that surfaced the loss.
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
///   straight onto the Filter's 80..8000 Hz Log2 throw as `hz = 80 * 100^f`,
///   which *is* that taper's position mapping, so its depths carried over
///   unchanged.
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
    "n1_Tk9PSQIADgAAAAIAlQAB5gPFAAJKMPtE",
    // Driving perc / full progression. Master and Kick Drive dialed back
    // (0.3->0.2, 0.56->0.4, catalog-typical levels) after the fast arp leg
    // read as overly distorted at their original settings.
    "n1_Tk9PSQIAiQAAABwAAAAAAAADAAMBBAADAgUAAwIGAAMIEAADBRMAAwEUAAMCMwAA9ig1AADpMZUAAYUBQwAA1yNFAADobkYAAFsJxQAC1m8YQ04AAAAAUAADBlIAAwFXAABSONwAAAAAWgAA1yNbAAAzM2MAAMxMZwACAAAAPmsAAwIMAQAzs0UCA_VGAgIAAJBAAUwAAAAEADUAAAAAQB8FAAAAAADCGMt-NgAAAABArgcAAAAAAL4jG1mVAAAAAEFhCgAAAIBAAAAAAFMAAAAAQOsRBgAAAABncstjAAAAAAAA",
    // Sparse pad-led breakdown.
    "n1_Tk9PSQIAcwAAABgAAAAAj8IBAABUUwIAAEhhAwADAgQAAwIFAAMEBgADCAgAALieCQAAAAAKAACZmQ4AAwMQAAMFEwADAhgAAwMaAAMFHQADAx4AAwJ8AAB6lDMAAFwPNQAAcTk2AAIAAIhAlQABQwHFAAJKMPtEcQAARUUBOwAAAAMACgAAAMA_pDAAAABAP9XNKX4zAAAAeEEfBQAAAAAAmaB_xJUAAACAPy4FAAAAgD4AAAAAAAAAAAAA",
    // Full-band build with driving kick/clap and busy arp.
    "n1_Tk9PSQIAyAAAACkAAAAAAAABAABUUwIAAEhhBAADAQUAAwQGAAMICAAACdcQAAMDGgAD_B4AAwMzAAAULjUAACgxNgACAAAAP5MAAwKUAABcD5UAAwCWAAMIlwAA__-YAAHVATkAALgeOwAAAAA8AABojj0AAwFBAAMArQAB7wGvAABwPUMAABQuRQAAS0JGAABZb0oAAP_nxQACrktbRMcAAOtRVwAAAABaAADXI2MAAK5HZAAAAFBlAABUQ2YAAwBnAAIAAIA-DgEDAg8BAApXAV0AAAAFAGUAAAAAQY8CBAAAAACWKStpDwEAAABBzQwEAAAAAHaXimk1AAAAQEEfBQAAAAAAwhjLfjMAAACAP3sUAwAAQD-ZoH_EmAAAAIBAaAoAAACAPwAAAAAAAAAAAAA",
    "n1_Tk9PSQIAewAAABoAAAAA69EBAABUUwIAAEhhBAADAgUAAwQGAAMICAAAhWsJAADrUQoAAGZmEAADABUAAwUaAAMEfAAAM7OVAAHmA8UAAkow-0RNAAB7FE8AABRyUAADA1MAAwFUAAMEVwAAAADcAAAehd4AAwLfAAA9CnEAAEhIdwADBQEqAAAAAgBPAAAAgEFcDwAAAAAA_60XCE0AAACAQCkcAgAAwD83UFCgAAAAAAAA",
    "n1_Tk9PSQIA0QAAACoAAAAA69EBAABUUwIAAEhhBAADAgUAAwQGAAMICAAAhWsJAADrUQoAAGZmEAADABUAAwUaAAMEfAAAM7MzAABcDzUAANIbNgACAAAAPpUAAdUBQwAAHwVIAAIAAAA_xQAC-e3QRE0AAD0KTwAAFHJQAAMDUwADAVQAAwRXAAAAANwAAB6F3gADAt8AAD0KWgAAuB5bAAB7FFwAAAtdYAAA-ZhiAABSOPQAAHsUYwAAUjhkAABxJmUAAP89ZwACAACAPgwBAFyPcQAASEh3AAMFAW4AAAAGAGUAAAAAQR8FAAAAAACWKStpQwAAAABAzQwCAAAAAPMyO4c1AAAAQEGPAgAAAAAAwhjLfpUAAABAQdQUAAAAAAAAAAAATwAAAIBBXA8AAAAAAP-tFwhNAAAAgEApHAIAAMA_N1BQoAAAAAAAAA",
    "n1_Tk9PSQIA0AAAACwAAAAAR-EBAAAJOwIAAOtEAwADAgQAAwIGAAMICAAAAIAOAAMBEAADARQAAwEYAAMBGgADAh0AAwEeAAMBHwADBSIAAwEkAAMCJwADASkAAwQsAAMBLgADBjEAAwIyAAMBfAAAAIAzAABcDzUAAH0WNgACAAAAP5UAAYAISAACAACAPsUAAvnt0ERaAAAzM1sAAKNwXAAACz1eAAMAYwAArkdlAACqSGsAAwIMAQAJ13EAAFRUcgAAM7N2AABmhjwCALgeRgICAAAgQEgCA30BTAAAAAQAZQAAAIBAPQoAAAAAQJYpK2lnAAAAAEDXIwYAAAAAGL6w-0MAAADAPwoXBAAAAADzMjuHNQAAAIBBjwIAAACAQMIYy34AAAAAAAA",
    "n1_Tk9PSQIArgAAACUAAAAAHoUBAAAAAAIAAAAABAADAQYAAwgLAAP-EAADABUAA_4aAAMAHwADAyIAAwEkAAMEJwADASkAAwAtAAMCLgADADEAAwEyAAMCMwAAexQ1AAB9FjYAAwE3AAIAAAA_lQABgAhDAABwPUUAAKBnRgAABCpHAAMCSwAAAADFAAJBi5BExwAAcD1OAAAAAE8AAKoqUgADAVMAAgAAgD5UAAMDVwAAAABxAACEhAE7AAAAAgBIAAAAAD-kMAcAAAAAINAEzgcAAP9__3__f_9__3-ZGWYGTwAAAIBBjwIAAAAAAP-tFwgAAAAAAAA",
    "n1_Tk9PSQIACQEAADcAAAAAPYoBAAAAAAIAAAAABAADAQYAAwgLAAP-EAADABUAA_4aAAMAHwADAyIAAwEkAAMEJwADASkAAwAtAAMCLgADADIAAwIzAAAfBTUAACcRNgADATcAAgAAAD-VAAHmAzkAAFG4OwAADwM8AADYnz0AAwFAAAMCrwAAj0JDAABwPUUAAKBnRgAABCpHAAMCSwAAAADFAAJBi5BExwAAcD1NAABwPU4AAAAgTwAAqipSAAMBUwACAACAPlQAAwNXAAAAANwAAD0KWgAAuB5bAADrUVwAALZnYAAAqlpiAAAAAGQAAMcLZQAA_m1mAAMCZwACAADAP3EAAISERQID80YCAgAAMEABbgAAAAUAQAAAAIBA4ToEAAAAACzI6VlcAAAAgEGZGQAAAAAA3JDYIkgAAAAAP6QwBwAAAAAg0ATOBwAA_3__f_9__3__f5kZZgZPAAAAgEGPAgAAAAAA_60XCE0AAACAQesRAAAAgEA3UFCgAAAAAAAAAkIAAAABDC0AAAA0AAAAOQAAADwAAAA5AAAANAAAADIAAAAwAAAAMgAAADcAAAA0AAAALQAAAK5ywNW3kSM8AAAAAAAAAAA",
    "n1_Tk9PSQIAFAEAADkAAAAAPYoBAAAAAAIAAAAABAADAQYAAwgLAAP-EAADABUAA_4aAAMAHwADAyIAAwEkAAMEJwADASkAAwAtAAMCLgADADIAAwIzAACPQjUAACcRNgADATcAAgAAAD-VAAKeG7dEOQAAAIA7AAAPAzwAAC2lPQADAUAAAwKvAACPQkMAAHA9RQAAoGdGAAAEKkcAAwJLAAAAAMUAAkGLkETHAABwPU0AAK5HTgAAACBPAACqKlIAAwFTAAIAAIA-VAADA1cAAAAA3AAAPQpaAADXI1sAAOtRXAAAtmdgAACqWmIAAAAAZAAAxwtlAABTY2YAAwJnAAIAAMA_awADAgwBAPYocQAAhIRFAgPzRgICAAAwQAGQAAAABwBAAAAAgEDhOgQAAAAALMjpWVwAAACAQZkZAAAAAADckNgiSAAAAAA_pDAHAAAAACDQBM4HAAD_f_9__3__f_9_mRlmBsUAAAAAQAAAAAAAAEBt_F8uAAAAAAA_AAAAAAAAALLa2jRPAAAAgEGPAgAAAAAA_60XCE0AAACAQesRAAAAgEA3UFCgAAAAAAAAAkIAAAABDC0AAAA0AAAAOQAAADwAAAA5AAAANAAAADIAAAAwAAAAMgAAADcAAAA0AAAALQAAAD5v5qhwOHCOAAAAAAAAAAA",
    "n1_Tk9PSQIAPAEAAEEAAAAArkcBAADBJwIAAGkuAwADAgUAAwQGAAMICAAAAAAJAAAAAAoAAAAADwADARAAAwUTAAMBFQADBBgAAwEaAAMHHQADAXwAAAAAMwAAexQ1AAB9FjYAAgAAAD-TAAMClAAAPQqVAAMAlgADCJcAAP__mAABhQE8AACDej0AAwFAAAMDrQABIAOvAADrUUMAAFwPRQAAoEdGAACuNEcAAwJIAAMCSgAA_9dLAABmJsUAAvRmNkTHAAAKV00AANcjTgAAAABPAAAUQlIAAwFTAAIAAMA_VAADA1cAAAAA3AAAMzNbAAC4HlwAAGFSZAAAHBFlAAD_TWYAAwdnAAIAAIA-agADAWsAAwIMAQBSOHEAACSkdgAAmZk8AgC4HkQCAMxMRQID9UYCAgAAIEBIAgGQAEkCAgAAYEABRgAAAAMAAAAAAIA_KRwCAAAAALLa2jQ1AAAAAEKPAgAAAAAAwhjLfjYAAACAPwCABwAAAAC-IxtZBGYm_5__f_-f_58AAAAAAAACQgAAAAEMMgAAADkAAABDAAAAMgAAADIAAAAwAAAAMgAAAD4AAAAyAAAAOQAAADIAAAA0AAAAeIwdDPPogs4GAAAAAAAAAA",
    "n1_Tk9PSQIAQgEAAEIAAAAArkcBAADBJwIAAGkuAwADAgUAAwQGAAMICAAAAAAJAAAAAAoAAAAADwADARAAAwUTAAMBFQADBBgAAwEaAAMHHQADAXwAAAAAMwAAexQ1AAB9FjYAAgAAAD-TAAMClAAAPQqVAAMAlgADCJcAAP__mAABhQE8AACDej0AAwFAAAMDrQABIAOvAADrUUMAAFwPRQAAoEdGAACuNEcAAwJIAAMCSgAA_9dLAABmJsUAAvRmNkTHAAAKV00AANcjTgAAAABPAAAUQlIAAwFTAAIAAMA_VAADA1cAAAAA3AAAMzNaAACZGVsAALgeXAAAYHJjAADXI2QAABwRZQAAqjhnAAIAAEA_agADAWsAAwIMAQBcj3EAACSkdgAAmZk8AgC4HkQCAMxMRQID9UYCAgAAIEBIAgGQAEkCAgAAYEABjAAAAAYAZQAAAABCHwUAAAAAAJYpK2lnAAAAAEAAgAcAAAAAGL6w-wgzM2ZG_3__f_9_zGz_f_-f_59cAAAAAEKZGQAAAAAA3JDYIgAAAACAPykcAgAAAACy2to0NQAAAABCjwIAAAAAAMIYy342AAAAgD8AgAcAAAAAviMbWQRmJv-f_3__n_-fAAAAAAAAAkIAAAABDDIAAAA5AAAAQwAAADIAAAAyAAAAMAAAADIAAAA-AAAAMgAAADkAAAAyAAAANAAAAHiMHQzz6ILOBgAAAAAAAAA",
    "n1_Tk9PSQIAEAEAADcAAAAAAAABAAAAAAIAAAAABgADBwgAAAAACQAAAAAKAAAAAHwAAAAAMwAAXA81AAB9FpMAAwKUAAA9CpUAAwCWAAMIlwAA__-YAAE1AjkAAHA9PAAAg3o9AAMBQAADA60AASADrwAA61FDAAB7FEUAAPVMRgAABEpHAAMCSgAA_8dLAABmJsUAAmDTg0THAAAKV04AAAAATwAAaTdSAAMCVAADA1cAAAAA3AAAMzNaAAC4HlsAAPYoXAAAYVJjAAB7FGQAABwRZQAAqlhmAAMHZwACAACAPmoAAwFrAAMCDAEAUjhxAAAtrXYAAJmZPAIAuB5EAgDMTEUCA_VGAgIAACBASAIBkABJAgIAAGBAAT0AAAACAAAAAAAAP-G6BwAAAACy2to0CI8C_3_-__9__v__f_9__7_-_zUAAAAAQo8CAAAAAADCGMt-AAAAAAAAAjIAAAACCDkAAAA8AAAAQAAAAD4AAAA8AAAAOQAAADQAAAA3AAAAl3Mb084gfRMAAAAAAAAAAA",
    "n1_Tk9PSQIABwEAADYAAAAAAAABAAAAAAIAAAAABAADAgUAAwQGAAMICAAAAAAJAAAAAAoAAAAAEAADAxoAA_weAAMDfAAAAAA1AAAoMTYAAgAAAD-TAAMClAAAPQqVAAMAlgADCJcAAP__mAABNQI5AAB7FDsAAAAAPAAAaI49AAMBQQADAK0AAecJrwAAUjhDAACZGUUAAEtCRgAAWW9IAAMCSgAA_99LAADNDMUAAmDTg0THAACuR00AAB8FVwAAAADcAAB6lN4AAwLfAAAfBVoAAFI4WwAAcD1iAACuR_QAANcjYwAAR2FkAAAAUGUAAFUjZgADAGcAAgAAgD4MAQDXIw4BAwIPAQDr0XEAAMFAAYEAAAAGAAAAAACAPv__BwAAAACy2to0CI8C_3_-__9__v__f_9__v__fzUAAABAQboDAAAAAADCGMt-MwAAAIA_6g4DAABAP5mgf8SYAAAAgECbBwAAAIA_AAAAAE8AAACAQS0EAAAAAAD_rRcITQAAAIBApwcCAADAPzdQUKAAAAAAAAACMgAAAAAILQAAADIAAAA3AAAAMAAAADQAAAA5AAAAMgAAADcAAADblhtoh_9H9gAAAAAAAAAA",
    "n1_Tk9PSQIABwEAADYAAAAA4XoBAABUUwIAAASFAwADAgQAAwIFAAMCBgADCAgAAHC9CQAAKVwKAAAUrhAAAwITAAMBFAADAjMAAJkZNQAAPic2AAIAAAA_kwADApQAAFI4lQADAJYAAwiXAAD__5gAAYUBOwAAuh08AADYzz0AAwGtAAHJA0MAAFwPRQAA6G5GAABbCcUAAtZvGEPHAABcj00AABQuTgAAVQVPAAAUQlAAAwZSAAMBVwAAXA_cAAAAAN4AAwLfAAAULloAADMzWwAAuB5cAAALXWQAABwhZQAA_y1nAAIAAAA-agADAWsAAwIMAQAzs3EAAMRDcgAArcc8AgBwPUUCA_NGAgIAAJhAAaMAAAAHAGUAAAAAQh8FAAAAAACWKStpZwAAAIA_AIAHAAAAABi-sPsIMzNmRv9__3__f8xs_3__n_-fQAAAAIBAAAAEAAAAACzI6VlcAAAAAEIKFwAAAAAA3JDYIkgAAAAAPwAABwAAAAAg0ATOBwAA_3__f_9__3__f5kZZgYAAAAAgD8pXAIAAAAAstraNDUAAAAAQh8FAAAAAADCGMt-AAAAAAAA",
    "n1_Tk9PSQIAqwAAACIAAAAAo_ABAABUUwIAAEhhBAADAgYAAwYJAACuRwoAAKNwfAAAj8IzAABcDzYAAgAAiECVAAEMAUMAALgeRQAA9UxGAAAEKkcAAwFIAAIAAAA_xQACrDODRU0AAJkZTgAA4URPAAAUclEAAwFSAAMBUwADAVQAAwhVAAIAAAA_VwAAAADcAADCdVoAANcjWwAAMzNxAABFRXIAAK3HPAIAexRFAgP0RgIDAwE7AAAAAwBDAAAAgD_rEQQAAIA-8zI7hzMAAABAQa4HAAAAAACZoH_ETwAAAABCzQwAAAAAAP-tFwgAAAAAAAA",
    "n1_Tk9PSQIAMAEAAD8AAAAAUbgBAAAJOwIAAOtEBAADAQUAAwYGAAMICAAAj8IQAAMAGgADAB8AAwIiAAMBJAADAycAAwJ8AAAJ1zMAANcjNQAA0kuVAAHmAzkAADMzOwAAuT09AAMBPgACAABAP0AAAwGtAAEuDa8AAOF6QwAAXA9GAABZH0cAAwNIAAIAAIA-SgAA_-dLAAAzM8cAAIVrTQAAXA9OAADhJE8AABRiUAADBlEAAwJSAAMFUwACAACAPlQAAgAAwD9VAAMBVwAAAADcAAD__94AAwLfAACZGVsAAOtRXQACAAAAP18AAwRgAABOrmIAAMJ19AAAHwVjAABSOGUAAKpIZgADB2cAAgAAgD5rAAMCDgEDAg8BAD0KPAIA9ihEAgCFa0UCA_FGAgMDSAIDPEkCAwUB4AAAAAsAZwAAAIBBM3MHAAAAABi-sPsCAAD_f_8_PAAAAEBBzQwAAAAAAC5ytLZaAAAAAD-jcAcAAAAAZ9M_OQQAAGXGZYYyk_-fSwAAAABAwjUAAAAAABUThSJDAAAAgD4pXAcAAAAA8zI7hwQAAJi5MpPMjP-fRQAAAIBBChcAAAAAAAO9h2_HAAAAAEC4HgAAAAAAOc8m8AAAAACAP9ajAgAAAACy2to0NQAAACBCPQoCAAAAAMIYy34zAAAAIEIKFwIAAAAAmaB_xE8AAAAAQR8FAwAAAAD_rRcIAAAAAAAAAlIAAAAFEEAAAAAwAAAALQAAADQAAAA5AAAANAAAAEMAAAAtAAAAQAAAADwAAAA8AAAALQAAADIAAAAtAAAAPAAAAEMAAADN9XcH5ywDuxYAAAAAAAAA",
    "n1_Tk9PSQIAaAAAABYAAAAAUbgBAAAJOwIAAOtEBAADAQUAAwYGAAMICAAAj8IQAAMAGgADAB8AAwIiAAMBJAADAycAAwJ8AAAJ15UAAeYDRQAASjJGAACuJEcAAwFIAAIAAIA-SgAA_8dLAAAzM8cAAOtRAVcAAAAEAEsAAAAAQMI1AAAAAAAVE4UiQwAAAIA-PUoHAAAAAPMyO4cEAACYuTKTzIz_n8cAAAAAQLgeAAAAAAA5zybwAAAAAIA_1qMCAAAAALLa2jQAAAAAAAACMgAAAAAILQAAADIAAAA3AAAAMAAAADQAAAA5AAAAMgAAADcAAADa8rvfPi9FpgAAAAAAAAAA",
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

/// Throttle granularity for the morph writer: one 1/8 note, i.e. half a beat.
const MORPH_TICK_BEATS: f64 = 0.5;

// ============================================================
// Morph model
//
// Every leg is HOLD then TRANSITION: the `from` state is held steady for the
// first `HOLD_FRACTION` of the leg, then the states cross over in the final
// stretch. Sections actually sit still instead of fading the whole time.
//
// During the transition each control moves by its own behavior, derived from
// its `ControlKind`:
//
//   Glide (Gain/Continuous) — lerp `from`→`to` across the transition window.
//       Levels glide too, so total audible energy always stays between the two
//       endpoints: a morph can never introduce silence the endpoints don't have.
//       Drum exits are the exception: perc, kick, and clap cut together on the
//       transition downbeat when the target has no drums. Kick also starts on
//       that downbeat, rather than fading in with the other drum voices.
//
//   Snap (Discrete/Timing)  — never interpolated; hold `from`, then hard-jump.
//       Structural params (progression + chord count/length + arp pattern) all
//       jump together on the transition downbeat ("one") as one atomic event.
//       Every other grid param staggers in at 8-bar offsets after it, in
//       registry order, so similar sections hard-switch rather than crossfade.
// ============================================================

/// Fraction of a leg spent holding the `from` state before the transition. The
/// transition gets the remaining third, so transition length ≈ half the hold.
const HOLD_FRACTION: f64 = 2.0 / 3.0;

/// Spacing between successive non-structural grid hard-switches, in bars.
const STAGGER_STEP_BARS: f64 = 8.0;

/// Control ids that hard-jump together on the transition downbeat, never
/// interpolated and never staggered against each other. A progression change
/// and the chord shape it implies must arrive as one musical event.
const STRUCTURAL_SNAP_IDS: &[&str] = &[
    "pad.progression",
    "pad.chord_count",
    "pad.chord_bars",
    "arp.pattern",
];

/// The drum voices, whose level ids (`Tab::level_id`) snap instead of glide.
const DRUM_TABS: [Tab; 3] = [Tab::Perc, Tab::Kick, Tab::Clap];

fn is_structural(spec_id: &str) -> bool {
    STRUCTURAL_SNAP_IDS.contains(&spec_id)
}

fn is_drum_level(spec_id: &str) -> bool {
    DRUM_TABS.iter().any(|tab| tab.level_id() == Some(spec_id))
}

fn snaps_drum_level(spec_id: &str, from: f32, to: f32) -> bool {
    Tab::Kick.level_id() == Some(spec_id) || (is_drum_level(spec_id) && from > 0.0 && to == 0.0)
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

/// (spec index into `all_specs()` order, jump offset in bars from the
/// transition downbeat) for every changed non-structural grid param on a leg,
/// staggered in registry order. Structural and glide params aren't listed.
fn stepped_offsets(from: &FluidControls, to: &FluidControls) -> Vec<(usize, f64)> {
    all_specs()
        .enumerate()
        .filter(|(_, spec)| matches!(spec.kind, ControlKind::Discrete | ControlKind::Timing))
        .filter(|(_, spec)| !is_structural(spec.id))
        .filter(|(_, spec)| (spec.get)(from) != (spec.get)(to))
        .enumerate()
        .map(|(order, (index, _))| (index, (order + 1) as f64 * STAGGER_STEP_BARS))
        .collect()
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
pub(crate) struct MorphState {
    endpoints: Vec<SongState>,
    morph_ids: Vec<Option<usize>>,
    bars: u32,
    /// Staggered hard-switch offsets for leg i -> i+1 (mod n), precomputed once.
    stepped: Vec<Vec<(usize, f64)>>,
    /// Engine beat the morph timeline is anchored to. Zero for the baked-in
    /// loop (which starts at beat 0); set to the toggle beat for a live start
    /// so the first leg begins from the current state, not mid-loop.
    origin_beat: f64,
    /// Override leg length for leg 0 only. `None` for the baked-in loop,
    /// where every leg (including 0) uses `bars`; `Some(LIVE_FIRST_LEG_BARS)`
    /// for a live toggle, so only the just-triggered leg is short.
    first_leg_bars: Option<u32>,
}

impl MorphState {
    pub(crate) fn new(endpoints: Vec<SongState>, bars: u32) -> Self {
        assert!(
            !endpoints.is_empty(),
            "auto-morph requires at least one state"
        );
        let n = endpoints.len();
        let stepped = (0..n)
            .map(|i| stepped_offsets(&endpoints[i].controls, &endpoints[(i + 1) % n].controls))
            .collect();
        Self {
            endpoints,
            morph_ids: (1..=n).map(Some).collect(),
            bars: bars.max(1),
            stepped,
            origin_beat: 0.0,
            first_leg_bars: None,
        }
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
        morph.first_leg_bars = Some(LIVE_FIRST_LEG_BARS);
        morph
    }

    /// Bars for `leg_index`: `first_leg_bars` for leg 0 when set, `bars`
    /// otherwise. Legs after 0 always use `bars`, even for a live toggle.
    fn leg_bars(&self, leg_index: i64) -> u32 {
        if leg_index == 0 {
            self.first_leg_bars.unwrap_or(self.bars)
        } else {
            self.bars
        }
    }

    fn leg_beats(&self, leg_index: i64) -> f64 {
        f64::from(self.leg_bars(leg_index)) * 4.0
    }

    /// Beat within `leg_index` at which the hold ends and the transition
    /// begins, snapped down to a bar downbeat so structural changes land on
    /// "one".
    fn leg_transition_start_beat(&self, leg_index: i64) -> f64 {
        (f64::from(self.leg_bars(leg_index)) * HOLD_FRACTION).floor() * 4.0
    }

    /// (from index, to index, t in [0,1), leg index) for the leg containing
    /// `beat`. Leg 0 may run a different length than the rest (a live
    /// toggle's short first leg); every leg after it is uniform, so once
    /// past leg 0 this steps at a fixed cadence. Looping A→B→…→A forever
    /// falls out of the modulo, correct for any N.
    fn leg_at_indexed(&self, beat: f64) -> (usize, usize, f64, i64) {
        let beat = (beat - self.origin_beat).max(0.0);
        let first_leg_beats = self.leg_beats(0);
        let (leg_index, within_leg) = if beat < first_leg_beats {
            (0, beat)
        } else {
            let steady_beats_per_leg = self.leg_beats(1);
            let rest = beat - first_leg_beats;
            let steps = (rest / steady_beats_per_leg).floor() as i64;
            (steps + 1, rest - steps as f64 * steady_beats_per_leg)
        };
        let beats_per_leg = self.leg_beats(leg_index);
        let t = (within_leg / beats_per_leg).clamp(0.0, 1.0);
        let n = self.endpoints.len() as i64;
        let from = leg_index.rem_euclid(n) as usize;
        let to = (leg_index + 1).rem_euclid(n) as usize;
        (from, to, t, leg_index)
    }

    /// (from index, to index, t in [0,1)) for the leg containing `beat`.
    #[cfg(test)]
    fn leg_at(&self, beat: f64) -> (usize, usize, f64) {
        let (from, to, t, _) = self.leg_at_indexed(beat);
        (from, to, t)
    }

    /// What is sounding at `beat`. A leg holds its `from` state for the first
    /// `HOLD_FRACTION` and only then crosses, so for most of a leg the honest
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

    /// The morphed `FluidControls` at `beat`: hold `from`, then glide or
    /// hard-switch each control across the transition window (see the module
    /// comment). At the leg boundary `leg_at` wraps to the next leg's `from`,
    /// which equals this leg's `to`, so the real target lands exactly on "one".
    pub(crate) fn controls_at(&self, beat: f64) -> FluidControls {
        let (from_idx, to_idx, t, leg_index) = self.leg_at_indexed(beat);
        let from = &self.endpoints[from_idx].controls;
        let to = &self.endpoints[to_idx].controls;
        let offsets = &self.stepped[from_idx];
        let beats_per_leg = self.leg_beats(leg_index);
        let t_beat = t * beats_per_leg;
        let transition_start = self.leg_transition_start_beat(leg_index);
        let transition_beats = (beats_per_leg - transition_start).max(1e-6);

        let mut next = from.clone();
        for (index, spec) in all_specs().enumerate() {
            let from_v = (spec.get)(from);
            let to_v = (spec.get)(to);

            let value = match spec.kind {
                ControlKind::Gain if snaps_drum_level(spec.id, from_v, to_v) => {
                    if t_beat < transition_start {
                        from_v
                    } else {
                        to_v
                    }
                }
                ControlKind::Gain | ControlKind::Continuous => {
                    let tt =
                        ((t_beat - transition_start) / transition_beats).clamp(0.0, 1.0) as f32;
                    from_v + (to_v - from_v) * tt
                }
                ControlKind::Timing | ControlKind::Discrete => {
                    let jump_beat = if is_structural(spec.id) {
                        transition_start
                    } else {
                        match offsets.iter().find(|(i, _)| *i == index) {
                            Some(&(_, offset_bars)) => {
                                (transition_start + offset_bars * 4.0).min(beats_per_leg)
                            }
                            None => transition_start, // unchanged: `from` and `to` are identical
                        }
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
    /// full rationale.
    pub(crate) fn automation_at(&self, beat: f64) -> AutomationState {
        let (from_idx, to_idx, t, leg_index) = self.leg_at_indexed(beat);
        let from = &self.endpoints[from_idx].automation;
        let to = &self.endpoints[to_idx].automation;
        let beats_per_leg = self.leg_beats(leg_index);
        let t_beat = t * beats_per_leg;
        let transition_start = self.leg_transition_start_beat(leg_index);
        let transition_beats = (beats_per_leg - transition_start).max(1e-6);
        let tt = ((t_beat - transition_start) / transition_beats).clamp(0.0, 1.0) as f32;
        AutomationState::morph(from, to, tt, t_beat >= transition_start)
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
    ) {
        if self.is_running() {
            self.exit();
        } else {
            let state = MorphState::from_live(
                current,
                current_automation,
                self.states.clone(),
                self.bars,
                beat,
            );
            self.morph.store(Arc::new(Some(state)));
        }
    }
}

/// Throttled writer driving the morph from the engine's control-reload tick.
/// Recomputes and returns the morphed controls and automation only once per
/// 1/8 note, tracking the last beat it fired on so the audio thread never
/// rewrites the shared Arcs more often than that.
#[derive(Default)]
pub(crate) struct MorphWriter {
    last_tick_beat: Option<f64>,
}

impl MorphWriter {
    /// `Some((controls, automation))` when a new morph tick is due at `beat`;
    /// `None` otherwise (call site should skip the write).
    pub(crate) fn tick(
        &mut self,
        morph: &MorphState,
        beat: f64,
    ) -> Option<(FluidControls, AutomationState)> {
        let due = match self.last_tick_beat {
            None => true,
            Some(last) => beat - last >= MORPH_TICK_BEATS,
        };
        if !due {
            return None;
        }
        self.last_tick_beat = Some(beat);
        Some((morph.controls_at(beat), morph.automation_at(beat)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (playing, next) for a beat, the pair the tests care about.
    fn ids(morph: &MorphState, beat: f64) -> (Option<usize>, Option<usize>) {
        let at = morph.position_at(beat);
        (at.playing, at.next)
    }

    fn state(bpm: f32) -> FluidControls {
        let mut c = FluidControls::default();
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
        // 2 bars/leg * 4 beats = 8 beats per leg.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(4.0), (0, 1, 0.5));
        assert_eq!(morph.leg_at(8.0), (1, 0, 0.0));
        assert_eq!(morph.leg_at(12.0), (1, 0, 0.5));
        assert_eq!(morph.leg_at(16.0), (0, 1, 0.0));
    }

    #[test]
    fn leg_math_eight_states_wraps_forever() {
        let endpoints: Vec<SongState> = (0..8)
            .map(|i| SongState::from_controls(state(80.0 + i as f32)))
            .collect();
        let morph = MorphState::new(endpoints, 1);
        // 1 bar/leg * 4 beats = 4 beats per leg.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(28.0), (7, 0, 0.0));
        assert_eq!(morph.leg_at(30.0), (7, 0, 0.5));
        assert_eq!(morph.leg_at(32.0), (0, 1, 0.0));
    }

    /// The footer reads this. Through the hold there is one song sounding and
    /// no transition to report; once the leg crosses, both ends and the
    /// progress between them are real.
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

        assert_eq!(ids(&morph, 4.0), (Some(2), Some(3)));
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
        let (from, to, t_end) = morph.leg_at(3.999_999);
        assert_eq!((from, to), (0, 1));
        assert!(t_end > 0.99);
    }

    #[test]
    fn gain_param_lerps_linearly() {
        // 1 bar/leg: no hold window, so the whole leg is a linear glide.
        let mut from = FluidControls::default();
        from.pad.level = 0.0;
        let mut to = FluidControls::default();
        to.pad.level = 1.0;
        let morph = MorphState::new(
            vec![SongState::from_controls(from), SongState::from_controls(to)],
            1,
        );
        let controls = morph.controls_at(4.0 * 0.25);
        assert!((controls.pad.level - 0.25).abs() < 1e-4);
    }

    #[test]
    fn drum_exits_cut_together_on_the_transition_downbeat() {
        let mut from = FluidControls::default();
        from.perc.level = 0.4;
        from.kick.level = 0.8;
        from.clap.level = 0.6;
        let to = FluidControls::default();
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
    fn kick_starts_on_the_transition_downbeat_while_other_drums_can_fade_in() {
        let from = FluidControls::default();
        let mut to = FluidControls::default();
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
        assert_eq!(mid.kick.level, 0.8);
        assert!((mid.clap.level - 0.3).abs() < 1e-4);
    }

    #[test]
    fn glide_holds_through_hold_window_then_lerps() {
        let mut from = FluidControls::default();
        from.modules.master[0].amount = 0.0;
        let mut to = FluidControls::default();
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

    #[test]
    fn structural_params_snap_together_on_transition_downbeat() {
        let from = FluidControls::default();
        let mut to = FluidControls::default();
        to.pad.progression = 3.0;
        to.pad.chord_count = 2.0;
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

        // On the downbeat: all three jump together, no interpolation.
        let after = morph.controls_at(16.0);
        assert_eq!(after.pad.progression, 3.0);
        assert_eq!(after.pad.chord_count, 2.0);
        assert_eq!(after.arp.pattern, 2.0);
    }

    #[test]
    fn nonstructural_grid_param_staggers_after_the_structural_downbeat() {
        let from = FluidControls::default();
        let mut to = FluidControls::default();
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
        let mut loud = FluidControls::default();
        loud.pad.level = 0.9;
        loud.kick.level = 0.8;
        loud.bass.level = 0.7;
        let mut quiet = FluidControls::default();
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
        let mut current = FluidControls::default();
        current.pad.level = 0.5;
        let mut far = FluidControls::default();
        far.pad.level = 1.0;
        far.kick.level = 1.0;
        far.bass.level = 1.0;
        let mut near = FluidControls::default();
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
        let mut current = FluidControls::default();
        current.pad.level = 0.2;
        let mut target = FluidControls::default();
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
    fn from_live_first_leg_finishes_transitioning_within_sixteen_bars() {
        let mut current = FluidControls::default();
        current.pad.level = 0.2;
        let mut target = FluidControls::default();
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
        let mut from_state = FluidControls::default();
        from_state.modules.master[0].amount = 0.0;
        let mut to_state = FluidControls::default();
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
                    ..SongState::from_controls(FluidControls::default())
                },
                SongState {
                    automation: to_auto,
                    ..SongState::from_controls(FluidControls::default())
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
                    ..SongState::from_controls(FluidControls::default())
                },
                SongState {
                    automation: unrouted.clone(),
                    ..SongState::from_controls(FluidControls::default())
                },
                SongState {
                    automation: unrouted,
                    ..SongState::from_controls(FluidControls::default())
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
            FluidControls::default(),
            current_auto.clone(),
            vec![SongState::from_controls(FluidControls::default())],
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
}
