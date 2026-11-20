use crate::model::Line;

pub fn current_line(position_ms: u64, lines: &[Line], offset_ms: i64) -> Option<Line> {
    let at = (position_ms.min(i64::MAX as u64) as i64).saturating_add(offset_ms);
    let end = lines.partition_point(|line| line.start_time_ms <= at);
    end.checked_sub(1).map(|index| lines[index].clone())
}

pub fn estimated_position(position: u64, elapsed: u64, playing: bool, duration: u64) -> u64 {
    // Stop extrapolating after a missed poll or sleep; the next observation corrects it.
    position
        .saturating_add(if playing { elapsed.min(1500) } else { 0 })
        .min(duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_pause_seek_and_stale_observation() {
        let lines = vec![
            Line {
                start_time_ms: 1000,
                original: "A quiet morning".into(),
                translated: None,
            },
            Line {
                start_time_ms: 2000,
                original: "A brighter sky".into(),
                translated: None,
            },
        ];
        assert!(current_line(999, &lines, 0).is_none());
        assert_eq!(
            current_line(2000, &lines, 0).unwrap().original,
            "A brighter sky"
        );
        assert_eq!(
            current_line(2000, &lines, -1000).unwrap().original,
            "A quiet morning"
        );
        assert!(current_line(0, &lines, 0).is_none());
        assert_eq!(estimated_position(1000, 9000, true, 10000), 2500);
        assert_eq!(estimated_position(1000, 9000, false, 10000), 1000);
    }
}
