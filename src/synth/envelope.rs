#[derive(Clone, Copy)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
    Done,
}

pub(crate) struct Adsr {
    attack_samples: f32,
    decay_samples: f32,
    sustain_level: f32,
    release_samples: f32,
    stage: Stage,
    stage_sample: f32,
    level: f32,
    release_start_level: f32,
}

impl Adsr {
    pub(crate) fn new(
        attack_seconds: f32,
        decay_seconds: f32,
        sustain_level: f32,
        release_seconds: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            attack_samples: attack_seconds * sample_rate,
            decay_samples: decay_seconds * sample_rate,
            sustain_level,
            release_samples: release_seconds * sample_rate,
            stage: Stage::Attack,
            stage_sample: 0.0,
            level: 0.0,
            release_start_level: 0.0,
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        match self.stage {
            Stage::Attack => {
                self.level = if self.attack_samples <= 1.0 {
                    1.0
                } else {
                    (self.stage_sample / self.attack_samples).min(1.0)
                };
                self.stage_sample += 1.0;
                if self.level >= 1.0 {
                    self.stage = Stage::Decay;
                    self.stage_sample = 0.0;
                }
            }
            Stage::Decay => {
                let progress = if self.decay_samples <= 1.0 {
                    1.0
                } else {
                    (self.stage_sample / self.decay_samples).min(1.0)
                };
                self.level = 1.0 + (self.sustain_level - 1.0) * progress;
                self.stage_sample += 1.0;
                if progress >= 1.0 {
                    self.stage = Stage::Sustain;
                    self.stage_sample = 0.0;
                }
            }
            Stage::Sustain => {
                self.level = self.sustain_level;
            }
            Stage::Release => {
                let progress = if self.release_samples <= 1.0 {
                    1.0
                } else {
                    (self.stage_sample / self.release_samples).min(1.0)
                };
                let curve = (1.0 - progress).powi(2);
                self.level = self.release_start_level * curve;
                self.stage_sample += 1.0;
                if progress >= 1.0 || self.level <= 0.0001 {
                    self.stage = Stage::Done;
                    self.level = 0.0;
                }
            }
            Stage::Done => {
                self.level = 0.0;
            }
        }

        self.level
    }

    pub(crate) fn note_off(&mut self) {
        if !self.is_done() && !matches!(self.stage, Stage::Release) {
            self.stage = Stage::Release;
            self.stage_sample = 0.0;
            self.release_start_level = self.level;
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }
}
