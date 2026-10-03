use std::time::Instant;

pub(crate) struct ScrollAcceleration {
    last_event: Option<Instant>,
    velocity: f64,
}

impl Default for ScrollAcceleration {
    fn default() -> Self {
        Self {
            last_event: None,
            velocity: 1.0,
        }
    }
}

impl ScrollAcceleration {
    pub(crate) fn factor(&mut self, now: Instant) -> f64 {
        if let Some(previous) = self.last_event {
            let elapsed = now.saturating_duration_since(previous).as_millis();
            if elapsed < 50 {
                self.velocity = (self.velocity + 0.8).min(5.0);
            } else if elapsed > 150 {
                self.velocity = 1.0;
            }
        }
        self.last_event = Some(now);
        self.velocity
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn rapid_scrolling_caps_and_idle_time_resets_velocity() {
        let start = Instant::now();
        let mut state = ScrollAcceleration::default();
        assert_eq!(state.factor(start), 1.0);
        for i in 1..=10 {
            state.factor(start + Duration::from_millis(i));
        }
        assert_eq!(state.factor(start + Duration::from_millis(11)), 5.0);
        assert_eq!(state.factor(start + Duration::from_millis(100)), 5.0);
        assert_eq!(state.factor(start + Duration::from_millis(251)), 1.0);
    }
}
