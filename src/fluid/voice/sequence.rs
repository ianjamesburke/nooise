//! Shared sixteen-step windows and Rest/Tie/Hit interpretation for melodic voices.

pub(crate) const SEQUENCE_STEP_COUNT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SequenceStep<T> {
    Rest,
    Tie,
    Hit(T),
}

impl SequenceStep<usize> {
    pub(crate) fn note(value: f32) -> Self {
        match value.round() as i32 {
            value if value < 0 => Self::Tie,
            0 => Self::Rest,
            value => Self::Hit(value as usize),
        }
    }
}

/// Pads offset their source window; note lanes offset their trigger grid.
/// Both use the same bounded step window and wrap ties within its live length.
#[derive(Clone, Copy)]
pub(crate) struct SequenceWindow {
    count: usize,
    offset: usize,
}

impl SequenceWindow {
    pub(crate) fn new(count: usize, offset: usize) -> Self {
        Self { count, offset }
    }

    pub(crate) fn slot(self, step: usize) -> usize {
        (self.offset + step) % SEQUENCE_STEP_COUNT
    }

    pub(crate) fn ties_after(self, step: usize, mut is_tie: impl FnMut(usize) -> bool) -> usize {
        (1..self.count)
            .take_while(|ahead| is_tie(self.slot((step + ahead) % self.count)))
            .count()
    }
}
