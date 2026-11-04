//! The one publication boundary for user-audible live state.
//!
//! `LiveSessionSnapshot` is an immutable generation of controls, automation,
//! tonal state, mute state, and the transport; every writer goes through `LiveSession::transact` so the
//! audio thread only ever reads one coherent generation.

use super::*;

#[derive(Clone)]
pub(crate) struct PendingSection {
    pub(crate) index: usize,
    pub(crate) target_beat: f64,
    pub(crate) snapshot: Arc<LiveSessionSnapshot>,
}

pub(crate) type MuteState = [bool; TAB_COUNT];

/// One coherent, immutable generation of every user-audible live-session
/// value shared by the UI and audio threads.
#[derive(Clone)]
pub(crate) struct LiveSessionSnapshot {
    pub(crate) generation: u64,
    /// Live-only invalidation for pending slot-targeted edits. Parameter
    /// changes leave it alone; replacing any slot kind advances it.
    pub(crate) module_topology_revision: u64,
    /// Live-only invalidation of palette targets when authored lanes change.
    pub(crate) automation_revision: u64,
    pub(crate) controls: FluidControls,
    pub(crate) automation: AutomationState,
    pub(crate) tonal_sequence: TonalSequenceState,
    pub(crate) muted: MuteState,
    /// Momentary left-hand holds (`q` = Kick+Bass Mute, `e` = Rise, `r` = Perc Roll).
    /// Live-only: held keys do not travel in song codes.
    pub(crate) hold_muted: [bool; HOLD_MUTE_COUNT],
    pub(crate) kick_bass_latched: bool,
    pub(crate) kick_bass_press_seconds: Option<f64>,
    /// A boundary action that changes the audible future of this session.
    pub(crate) planned: Option<PlannedAction>,
    pub(crate) pending_section: Option<PendingSection>,
    pub(crate) current_section: Option<usize>,
    pub(crate) section_revision: u64,
    /// Active chord vamp (`a`), locking Pads, Bass, Arp, and Lead to the nearest
    /// chord start until released into the next chord in the progression.
    pub(crate) vamp: Option<VampState>,
    /// When `a` was physically pressed down, in audio seconds. Live-only.
    pub(crate) vamp_press_seconds: Option<f64>,
    /// Whether the vamp is latched hands-free (via a tap or `/vamp`) vs held. Live-only.
    pub(crate) vamp_latched: bool,
    /// Lead play-mode presses. Live-only: never written to a song code.
    pub(crate) lead_play: LeadPlayState,
    pub(crate) gestures: GestureState,
    /// Drunken phrase position at song beat zero, carried through saves.
    pub(crate) drunken_phase_beat: f64,
    /// Whether the beat clock runs. Live-only: a stopped song is silence,
    /// not a state worth sharing, so a loaded code always plays.
    pub(crate) transport: Transport,
    /// Counts explicit restarts, including stop/play between audio reads.
    pub(crate) transport_restart: u64,
    /// How chords are written on screen. Live-only: a display choice, never
    /// read by the audio thread or written to a song code.
    pub(crate) chord_names: ChordNames,
}

impl LiveSessionSnapshot {
    /// How this generation writes chord names: its display choice and the
    /// Master Key that letters are written in.
    pub(crate) fn chord_labels(&self) -> ChordLabels {
        ChordLabels {
            names: self.chord_names,
            key: MasterKey::from_control(self.controls.master.key),
        }
    }

    pub(crate) fn from_song(song: &SongState) -> Self {
        let mut controls = song.controls.clone();
        if let Some(sequence) = &song.tonal_sequence
            && sequence.evolution_count > 0
        {
            for (dst, &note) in controls.tonal.steps.iter_mut().zip(&sequence.notes) {
                *dst = note as f32;
            }
        }
        Self {
            generation: 0,
            module_topology_revision: 0,
            automation_revision: 0,
            controls,
            automation: song.automation.clone(),
            muted: song.muted,
            hold_muted: [false; HOLD_MUTE_COUNT],
            kick_bass_latched: song.kick_bass_latched,
            kick_bass_press_seconds: None,
            planned: song.planned,
            pending_section: None,
            current_section: None,
            section_revision: 0,
            vamp: song.vamp,
            vamp_press_seconds: None,
            vamp_latched: song.vamp.is_some(),
            lead_play: LeadPlayState::default(),
            gestures: song.gestures.restored(),
            drunken_phase_beat: song.drunken_phase_beat,
            transport: Transport::Playing,
            transport_restart: 0,
            chord_names: ChordNames::default(),
            tonal_sequence: song
                .tonal_sequence
                .clone()
                .unwrap_or_else(|| TonalSequenceState::from_controls(&song.controls.tonal)),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_controls(controls: FluidControls) -> Self {
        Self {
            generation: 0,
            module_topology_revision: 0,
            automation_revision: 0,
            tonal_sequence: TonalSequenceState::from_controls(&controls.tonal),
            controls,
            automation: AutomationState::default(),
            muted: [false; TAB_COUNT],
            hold_muted: [false; HOLD_MUTE_COUNT],
            kick_bass_latched: false,
            kick_bass_press_seconds: None,
            planned: None,
            pending_section: None,
            current_section: None,
            section_revision: 0,
            vamp: None,
            vamp_press_seconds: None,
            vamp_latched: false,
            lead_play: LeadPlayState::default(),
            gestures: GestureState::default(),
            drunken_phase_beat: 0.0,
            transport: Transport::Playing,
            transport_restart: 0,
            chord_names: ChordNames::default(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct LiveSession {
    published: Arc<ArcSwap<LiveSessionSnapshot>>,
    /// Audio-owned monotonic clock; gestures store their anchors in the
    /// aggregate snapshot, while readers sample this clock without a lock.
    audio_seconds_bits: Arc<AtomicU64>,
    audio_beat_bits: Arc<AtomicU64>,
    audio_phrase_start_bits: Arc<AtomicU64>,
    /// Once rendering has begun, the audio clock owns exact-boundary actions.
    audio_clock_active: Arc<AtomicBool>,
}

impl LiveSession {
    pub(crate) fn new(snapshot: LiveSessionSnapshot) -> Self {
        Self {
            published: Arc::new(ArcSwap::from_pointee(snapshot)),
            audio_seconds_bits: Arc::new(AtomicU64::new(0)),
            audio_beat_bits: Arc::new(AtomicU64::new(0)),
            audio_phrase_start_bits: Arc::new(AtomicU64::new(0)),
            audio_clock_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn load(&self) -> Arc<LiveSessionSnapshot> {
        self.published.load_full()
    }

    pub(crate) fn audio_seconds(&self) -> f64 {
        f64::from_bits(self.audio_seconds_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn audio_beat(&self) -> f64 {
        f64::from_bits(self.audio_beat_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_audio_beat(&self, beat: f64) {
        self.audio_clock_active.store(true, Ordering::Relaxed);
        self.audio_beat_bits
            .store(beat.to_bits(), Ordering::Relaxed);
    }

    /// Whether an audio callback has started and therefore owns musical
    /// boundaries more precisely than the terminal scheduler can.
    pub(crate) fn audio_clock_active(&self) -> bool {
        self.audio_clock_active.load(Ordering::Relaxed)
    }

    pub(crate) fn audio_phrase_start(&self) -> f64 {
        f64::from_bits(self.audio_phrase_start_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_audio_phrase_start(&self, beat: f64) {
        self.audio_phrase_start_bits
            .store(beat.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn publish_audio_seconds(&self, seconds: f64) {
        self.audio_seconds_bits
            .store(seconds.to_bits(), Ordering::Relaxed);
    }

    /// Replace the entire authored song without applying edit-time lane invalidation.
    pub(crate) fn land_section(&self, pending: &PendingSection, now_seconds: f64) -> bool {
        {
            let current = self.published.load_full();
            if current.transport != Transport::Playing
                || !current
                    .pending_section
                    .as_ref()
                    .is_some_and(|p| Arc::ptr_eq(&p.snapshot, &pending.snapshot))
            {
                return false;
            }
            let mut next = pending.snapshot.as_ref().clone();
            next.generation = current.generation.wrapping_add(1);
            next.module_topology_revision = current.module_topology_revision.wrapping_add(1);
            next.automation_revision = current.automation_revision.wrapping_add(1);
            next.chord_names = current.chord_names;
            next.gestures = pending.snapshot.gestures.restored_at(now_seconds);
            // Section state replaces authored latches, but keys physically held
            // across the boundary remain under the performer's finger.
            next.hold_muted = current.hold_muted;
            next.kick_bass_press_seconds = current.kick_bass_press_seconds;
            for kind in GestureKind::ALL {
                let held = current.gestures.envelope(kind);
                if held.held && !held.restored {
                    next.gestures.lanes[kind as usize] = *held;
                }
            }
            if current.gestures.rise.held && !current.gestures.rise.restored {
                next.gestures.rise = current.gestures.rise;
            }
            // A held vamp follows the new section's first chord. Its physical
            // release still uses the original press time and destination grid.
            if current.vamp_press_seconds.is_some() && current.vamp.is_some() {
                next.vamp = Some(VampState::start(
                    0.0,
                    ChordWindow::requested(&next.controls.pad).offset as u8,
                ));
                next.vamp_press_seconds = current.vamp_press_seconds;
                next.vamp_latched = false;
            }
            next.current_section = Some(pending.index);
            next.section_revision = current.section_revision.wrapping_add(1);
            next.transport_restart = current.transport_restart.wrapping_add(1);
            let next = Arc::new(next);
            let previous = self.published.compare_and_swap(&current, next);
            Arc::ptr_eq(&previous, &current)
        }
    }

    /// Apply a pure aggregate edit with optimistic retry. A conflicting writer
    /// causes the edit to be recomputed from the newer generation, preventing
    /// stale clone-and-store updates from overwriting one another.
    pub(crate) fn transact<E>(
        &self,
        mut edit: impl FnMut(&mut LiveSessionSnapshot) -> Result<(), E>,
    ) -> Result<Arc<LiveSessionSnapshot>, E> {
        self.transact_with_before(|_, snapshot| edit(snapshot))
            .map(|(_, after)| after)
    }

    /// Return the exact Arc used by the successful CAS attempt, including
    /// after retries. Observers must compare this pair, not a separate load.
    pub(crate) fn transact_with_before<E>(
        &self,
        mut edit: impl FnMut(&LiveSessionSnapshot, &mut LiveSessionSnapshot) -> Result<(), E>,
    ) -> Result<(Arc<LiveSessionSnapshot>, Arc<LiveSessionSnapshot>), E> {
        loop {
            let current = self.published.load_full();
            let mut next = current.as_ref().clone();
            edit(&current, &mut next)?;
            if Tab::all().iter().any(|&tab| {
                current
                    .controls
                    .modules
                    .for_tab(tab)
                    .zip(next.controls.modules.for_tab(tab))
                    .is_some_and(|(before, after)| {
                        before
                            .iter()
                            .zip(after)
                            .any(|(before, after)| before.kind.to_bits() != after.kind.to_bits())
                    })
            }) {
                next.module_topology_revision = current.module_topology_revision.wrapping_add(1);
                next.automation.captures.retain(|address, _| {
                    let Some((before, _)) = module_slot_row(address.id(), &current.controls) else {
                        return true;
                    };
                    module_slot_row(address.id(), &next.controls)
                        .is_some_and(|(after, _)| before.kind == after.kind)
                });
                next.automation.editor_captures.retain(|target, _| {
                    let Some((before, _)) = module_slot_row(target.control.id(), &current.controls)
                    else {
                        return true;
                    };
                    module_slot_row(target.control.id(), &next.controls)
                        .is_some_and(|(after, _)| before.kind == after.kind)
                });
            }
            if !current.automation.same_lanes(&next.automation) {
                next.automation_revision = current.automation_revision.wrapping_add(1);
            }
            next.generation = current.generation.wrapping_add(1);
            let next = Arc::new(next);
            let previous = self.published.compare_and_swap(&current, Arc::clone(&next));
            if Arc::ptr_eq(&previous, &current) {
                return Ok((current, next));
            }
        }
    }

    pub(crate) fn update(
        &self,
        mut edit: impl FnMut(&mut LiveSessionSnapshot),
    ) -> Arc<LiveSessionSnapshot> {
        self.transact::<std::convert::Infallible>(|snapshot| {
            edit(snapshot);
            Ok(())
        })
        .expect("infallible live-session transaction")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::thread;

    #[test]
    fn section_landing_preserves_live_holds_and_uses_destination_latches() {
        let mut current = LiveSessionSnapshot::from_controls(FluidControls::default());
        current.hold_muted = [true, true, true];
        current.kick_bass_press_seconds = Some(2.0);
        current.kick_bass_latched = false;
        current.gestures.press(GestureKind::Submerge, 2.0);
        current.gestures.press_rise(2.0);
        current.gestures.lanes[GestureKind::Echo as usize] = GestureEnvelope {
            amount: 0.7,
            at_seconds: 0.0,
            held: true,
            restored: true,
        };
        let mut destination = LiveSessionSnapshot::from_controls(FluidControls::default());
        destination.kick_bass_latched = true;
        destination.muted[Tab::Kick as usize] = true;
        destination.gestures.lanes[GestureKind::Freeze as usize] = GestureEnvelope {
            amount: 0.4,
            at_seconds: 0.0,
            held: true,
            restored: true,
        };
        let pending = PendingSection {
            index: 1,
            target_beat: 4.0,
            snapshot: Arc::new(destination),
        };
        current.pending_section = Some(pending.clone());
        let session = LiveSession::new(current);
        assert!(session.land_section(&pending, 5.0));
        let landed = session.load();
        assert_eq!(landed.hold_muted, [true, true, true]);
        assert_eq!(landed.kick_bass_press_seconds, Some(2.0));
        assert!(landed.kick_bass_latched);
        assert!(landed.muted[Tab::Kick as usize]);
        assert!(!landed.muted[Tab::Bass as usize]);
        assert!(landed.gestures.envelope(GestureKind::Submerge).held);
        assert!(!landed.gestures.envelope(GestureKind::Submerge).restored);
        assert_eq!(
            landed.gestures.envelope(GestureKind::Submerge).at_seconds,
            2.0
        );
        assert!(landed.gestures.rise.held && !landed.gestures.rise.restored);
        assert!(!landed.gestures.envelope(GestureKind::Echo).held);
        assert!(landed.gestures.envelope(GestureKind::Freeze).restored);
        assert_eq!(
            landed.gestures.envelope(GestureKind::Freeze).at_seconds,
            5.0
        );
    }

    #[test]
    fn release_after_section_landing_keeps_destination_group_latch_and_layer_mutes() {
        use super::super::interaction::InteractionEffect;
        let mut current = LiveSessionSnapshot::from_controls(FluidControls::default());
        current.hold_muted[HoldMute::KickAndBass as usize] = true;
        current.kick_bass_press_seconds = Some(1.0);
        let mut destination = LiveSessionSnapshot::from_controls(FluidControls::default());
        destination.kick_bass_latched = true;
        destination.muted[Tab::Bass as usize] = true;
        let pending = PendingSection {
            index: 0,
            target_beat: 4.0,
            snapshot: Arc::new(destination),
        };
        current.pending_section = Some(pending.clone());
        let session = LiveSession::new(current);
        assert!(session.land_section(&pending, 2.0));
        session.publish_audio_seconds(2.0);
        let mut effects = EffectExecutor::seeded(
            session.clone(),
            AutoControls::new(no_morph(), Vec::new(), 64),
            42,
        );
        let mut flipped = FlippedUnits::default();
        let mut context = ProductionInteractionContext {
            selected_control: None,
            visible_control_ids: &[],
            randomizes_automation: false,
            tab: Tab::Master,
            selected: None,
            automation_selected: 0,
            beat: 0.0,
            flipped: &mut flipped,
        };
        let mut clipboard = SystemClipboard;
        let results = effects.execute_production_interactions_with_clipboard(
            [InteractionEffect::HoldMuteRelease(HoldMute::KickAndBass)],
            &mut context,
            &mut clipboard,
        );
        assert!(results.into_iter().all(|result| result.is_ok()));
        let after = session.load();
        assert!(!after.hold_muted[HoldMute::KickAndBass as usize]);
        assert!(after.kick_bass_latched);
        assert!(after.muted[Tab::Bass as usize]);
        assert!(!after.muted[Tab::Kick as usize]);
        assert_eq!(after.kick_bass_press_seconds, None);
    }

    #[test]
    fn stopped_transport_does_not_land_a_pending_section() {
        let mut current = LiveSessionSnapshot::from_controls(FluidControls::default());
        let destination = LiveSessionSnapshot::from_controls(FluidControls::default());
        let pending = PendingSection {
            index: 0,
            target_beat: 4.0,
            snapshot: Arc::new(destination),
        };
        current.pending_section = Some(pending.clone());
        current.transport = Transport::Stopped;
        let session = LiveSession::new(current);
        assert!(!session.land_section(&pending, 1.0));
        let after = session.load();
        assert_eq!(after.transport, Transport::Stopped);
        assert!(after.pending_section.is_some());
    }

    #[test]
    fn section_landing_moves_physical_vamp_to_destination_first_chord() {
        let mut current = LiveSessionSnapshot::from_controls(FluidControls::default());
        current.vamp = Some(VampState::start(12.0, 0));
        current.vamp_press_seconds = Some(3.0);
        let mut destination = LiveSessionSnapshot::from_controls(FluidControls::default());
        destination.controls.pad.chord_offset = 3.0;
        let pending = PendingSection {
            index: 0,
            target_beat: 16.0,
            snapshot: Arc::new(destination),
        };
        current.pending_section = Some(pending.clone());
        let session = LiveSession::new(current);
        assert!(session.land_section(&pending, 4.0));
        let landed = session.load();
        assert_eq!(landed.vamp, Some(VampState::start(0.0, 3)));
        assert_eq!(landed.vamp_press_seconds, Some(3.0));
        assert!(!landed.vamp_latched);
    }

    #[test]
    fn section_landing_uses_destination_vamp_without_a_physical_press() {
        let mut current = LiveSessionSnapshot::from_controls(FluidControls::default());
        current.vamp = Some(VampState::start(8.0, 1));
        current.vamp_latched = true;
        let mut destination = LiveSessionSnapshot::from_controls(FluidControls::default());
        destination.vamp = Some(VampState::start(0.0, 2));
        destination.vamp_latched = true;
        let pending = PendingSection {
            index: 0,
            target_beat: 16.0,
            snapshot: Arc::new(destination),
        };
        current.pending_section = Some(pending.clone());
        let session = LiveSession::new(current);
        assert!(session.land_section(&pending, 4.0));
        let landed = session.load();
        assert_eq!(landed.vamp, Some(VampState::start(0.0, 2)));
        assert!(landed.vamp_latched);
        assert_eq!(landed.vamp_press_seconds, None);
    }

    #[test]
    fn transaction_pair_retains_the_successful_retry_arc() {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let original = session.load();
        let mut intervening = None;
        let mut attempts = 0;
        let (before, after) = session
            .transact_with_before::<std::convert::Infallible>(|current, next| {
                attempts += 1;
                if attempts == 1 {
                    intervening =
                        Some(session.update(|snapshot| snapshot.controls.master.bpm = 99.0));
                }
                next.controls.pad.level = current.controls.pad.level / 2.0;
                Ok(())
            })
            .unwrap();
        assert_eq!(attempts, 2);
        assert!(Arc::ptr_eq(&before, intervening.as_ref().unwrap()));
        assert!(!Arc::ptr_eq(&before, &original));
        assert!(Arc::ptr_eq(&after, &session.load()));
        assert_eq!(after.controls.master.bpm, 99.0);
        assert_eq!(after.generation, before.generation + 1);
        assert_eq!(after.controls.pad.level, original.controls.pad.level / 2.0);
    }

    #[test]
    fn module_topology_revision_tracks_replacement_but_not_parameter_edits() {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        session.update(|snapshot| snapshot.controls.modules.bass[0].time = 1234.0);
        assert_eq!(session.load().module_topology_revision, 0);
        session.update(|snapshot| snapshot.controls.modules.bass[0] = ModuleSlot::default());
        session.update(|snapshot| snapshot.controls.modules.bass[0] = preset_slot("filter", 1.0));
        assert_eq!(session.load().module_topology_revision, 2);
    }

    use super::*;

    #[test]
    fn transaction_publishes_the_complete_aggregate_once() {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let before = session.load();

        let published = session.update(|snapshot| {
            snapshot.controls.master.bpm = 91.0;
            snapshot
                .automation
                .open_or_create(ControlAddress::new("master.bpm"));
            snapshot.tonal_sequence.evolution_count = 7;
            snapshot.muted[Tab::Master as usize] = true;
        });

        assert_eq!(published.generation, before.generation + 1);
        assert_eq!(published.controls.master.bpm, 91.0);
        assert!(
            published
                .automation
                .route(ControlAddress::new("master.bpm"))
                .is_some()
        );
        assert_eq!(published.tonal_sequence.evolution_count, 7);
        assert!(published.muted[Tab::Master as usize]);
        assert!(Arc::ptr_eq(&published, &session.load()));
    }

    #[test]
    fn failed_transaction_does_not_publish() {
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let before = session.load();
        let result = session.transact(|snapshot| {
            snapshot.controls.master.bpm = 200.0;
            Err::<(), _>("rejected")
        });

        assert!(matches!(result, Err("rejected")));
        assert!(Arc::ptr_eq(&before, &session.load()));
    }

    #[test]
    fn concurrent_writers_do_not_lose_updates_or_mix_generations() {
        const WRITES: usize = 100;
        let session =
            LiveSession::new(LiveSessionSnapshot::from_controls(FluidControls::default()));
        let barrier = Arc::new(Barrier::new(3));
        let mut writers = Vec::new();
        for writer in 0..2 {
            let session = session.clone();
            let barrier = Arc::clone(&barrier);
            writers.push(thread::spawn(move || {
                barrier.wait();
                for _ in 0..WRITES {
                    session.update(|snapshot| {
                        if writer == 0 {
                            snapshot.controls.master.bpm += 1.0;
                        } else {
                            snapshot.tonal_sequence.evolution_count += 1;
                        }
                    });
                }
            }));
        }
        barrier.wait();
        for writer in writers {
            writer.join().unwrap();
        }

        let snapshot = session.load();
        assert_eq!(snapshot.generation, (WRITES * 2) as u64);
        assert_eq!(
            snapshot.controls.master.bpm,
            FluidControls::default().master.bpm + WRITES as f32
        );
        assert_eq!(snapshot.tonal_sequence.evolution_count, WRITES as u64);
    }
}
