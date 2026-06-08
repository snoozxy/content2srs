use crate::subtitles::Cue;

#[derive(Debug, Clone)]
pub struct CardJob {
    pub card_id: String,
    pub cue_index: usize,
    pub start_ms: u64,
    pub end_ms: u64,
    pub screenshot_ms: u64,
    pub text: String,
    pub context: String,
}

#[derive(Debug, Clone)]
pub struct JobPlan {
    pub cards: Vec<CardJob>,
}

impl JobPlan {
    pub fn from_cues(cues: &[Cue], screenshot_offset_ms: u64) -> Self {
        let cards = cues
            .iter()
            .map(|cue| CardJob {
                card_id: format!("card_{:05}", cue.index),
                cue_index: cue.index,
                start_ms: cue.start_ms,
                end_ms: cue.end_ms,
                screenshot_ms: cue.start_ms.saturating_add(screenshot_offset_ms),
                text: cue.text.clone(),
                context: cue.context.clone(),
            })
            .collect();
        Self { cards }
    }

    pub fn audio_ranges(&self) -> Vec<(String, u64, u64)> {
        self.cards
            .iter()
            .map(|c| (c.card_id.clone(), c.start_ms, c.end_ms))
            .collect()
    }

    pub fn screenshot_times(&self) -> Vec<(String, u64)> {
        let mut times: Vec<(String, u64)> = self
            .cards
            .iter()
            .map(|c| (c.card_id.clone(), c.screenshot_ms))
            .collect();
        times.sort_by_key(|(_, t)| *t);
        times
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_sorted_screenshots() {
        let cues = vec![
            Cue {
                index: 0,
                start_ms: 5000,
                end_ms: 6000,
                text: "b".into(),
                context: String::new(),
            },
            Cue {
                index: 1,
                start_ms: 1000,
                end_ms: 2000,
                text: "a".into(),
                context: String::new(),
            },
        ];
        let plan = JobPlan::from_cues(&cues, 0);
        let times = plan.screenshot_times();
        assert_eq!(times[0].1, 1000);
        assert_eq!(times[1].1, 5000);
    }
}
