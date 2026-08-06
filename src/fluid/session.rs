use super::*;
use crate::fx::delay::StereoDelayState;
use crate::fx::reverb::FreeverbState;
use std::sync::atomic::{AtomicU64, Ordering};

/// Audio-thread state for all module delay lines. Empty entries are inactive
/// slots and cost nothing in a song snapshot.
#[derive(Clone)]
pub(crate) enum ModuleFxRuntimeSlot {
    Empty,
    Delay(StereoDelayState),
    Reverb(FreeverbState),
    Compression { envelope: f32 },
}

#[derive(Clone, Default)]
pub(crate) struct ModuleFxRuntimeState {
    pub(crate) slots: Vec<ModuleFxRuntimeSlot>,
}

#[derive(Clone, Default)]
pub(crate) struct ModuleFxCapture {
    pub(crate) request: u64,
    pub(crate) state: ModuleFxRuntimeState,
}

/// One coherent, immutable generation of every user-audible live-session
/// value shared by the UI and audio threads.
#[derive(Clone)]
pub(crate) struct LiveSessionSnapshot {
    pub(crate) generation: u64,
    pub(crate) controls: FluidControls,
    pub(crate) automation: AutomationState,
    pub(crate) tonal_sequence: TonalSequenceState,
    pub(crate) module_fx_runtime: Option<ModuleFxRuntimeState>,
}

impl LiveSessionSnapshot {
    pub(crate) fn from_song(song: &SongState) -> Self {
        Self {
            generation: 0,
            controls: song.controls.clone(),
            automation: song.automation.clone(),
            tonal_sequence: song.tonal_sequence.clone().unwrap_or_else(|| {
                TonalSequenceState::from_phrase(tonal_phrase_index(song.controls.tonal.phrase))
            }),
            module_fx_runtime: song.module_fx_runtime.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_controls(controls: FluidControls) -> Self {
        Self {
            generation: 0,
            tonal_sequence: TonalSequenceState::from_phrase(tonal_phrase_index(
                controls.tonal.phrase,
            )),
            controls,
            automation: AutomationState::default(),
            module_fx_runtime: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct LiveSession {
    published: Arc<ArcSwap<LiveSessionSnapshot>>,
    module_fx_capture_request: Arc<AtomicU64>,
    module_fx_capture: Arc<ArcSwap<ModuleFxCapture>>,
}

impl LiveSession {
    pub(crate) fn new(snapshot: LiveSessionSnapshot) -> Self {
        let module_fx_runtime = snapshot.module_fx_runtime.clone().unwrap_or_default();
        Self {
            published: Arc::new(ArcSwap::from_pointee(snapshot)),
            module_fx_capture_request: Arc::new(AtomicU64::new(0)),
            module_fx_capture: Arc::new(ArcSwap::from_pointee(ModuleFxCapture {
                request: 0,
                state: module_fx_runtime,
            })),
        }
    }

    pub(crate) fn load(&self) -> Arc<LiveSessionSnapshot> {
        self.published.load_full()
    }

    pub(crate) fn request_module_fx_capture(&self) -> u64 {
        self.module_fx_capture_request
            .fetch_add(1, Ordering::AcqRel)
            + 1
    }

    pub(crate) fn pending_module_fx_capture(&self) -> u64 {
        self.module_fx_capture_request.load(Ordering::Acquire)
    }

    pub(crate) fn publish_module_fx_capture(&self, request: u64, state: ModuleFxRuntimeState) {
        self.module_fx_capture
            .store(Arc::new(ModuleFxCapture { request, state }));
    }

    pub(crate) fn module_fx_capture(&self) -> Arc<ModuleFxCapture> {
        self.module_fx_capture.load_full()
    }

    /// Apply a pure aggregate edit with optimistic retry. A conflicting writer
    /// causes the edit to be recomputed from the newer generation, preventing
    /// stale clone-and-store updates from overwriting one another.
    pub(crate) fn transact<E>(
        &self,
        mut edit: impl FnMut(&mut LiveSessionSnapshot) -> Result<(), E>,
    ) -> Result<Arc<LiveSessionSnapshot>, E> {
        loop {
            let current = self.published.load_full();
            let mut next = current.as_ref().clone();
            edit(&mut next)?;
            next.generation = current.generation.wrapping_add(1);
            let next = Arc::new(next);
            let previous = self.published.compare_and_swap(&current, Arc::clone(&next));
            if Arc::ptr_eq(&previous, &current) {
                return Ok(next);
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
