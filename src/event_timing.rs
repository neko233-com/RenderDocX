use crate::replay::{Action, CounterDescription, CounterEntry};
use std::collections::HashMap;

/// EventGPUDuration is counter 1 in the matched upstream RenderDoc API.
pub const GPU_DURATION_COUNTER: u32 = 1;

pub fn available(counters: &[CounterDescription]) -> bool {
    counters.iter().any(|c| {
        c.id == GPU_DURATION_COUNTER
            && c.result_type == 1
            && c.result_byte_width == 8
            && c.unit == 1
    })
}

/// Match EventBrowser's marker totals: a parent's duration is the sum of its
/// direct children's nonnegative finite durations, after computing descendants.
pub fn aggregate(
    actions: &[Action],
    entries: &[CounterEntry],
) -> Result<HashMap<u32, f64>, String> {
    let mut times = HashMap::new();
    for entry in entries {
        if entry.counter != GPU_DURATION_COUNTER {
            return Err("Unexpected GPU duration counter".into());
        }
        let bits = entry
            .bits
            .parse::<u64>()
            .map_err(|_| "Invalid GPU duration bits")?;
        let seconds = f64::from_bits(bits);
        if seconds.is_finite() && seconds >= 0.0 {
            times.insert(entry.event_id, seconds);
        }
    }
    let mut sums = vec![0.0f64; actions.len()];
    let mut has_children = vec![false; actions.len()];
    for (index, action) in actions.iter().enumerate().rev() {
        if has_children[index] {
            times.insert(action.event_id, sums[index]);
        }
        if let Ok(parent) = usize::try_from(action.parent) {
            if parent < index {
                has_children[parent] = true;
                if let Some(seconds) = times.get(&action.event_id).copied() {
                    if seconds.is_finite() && seconds >= 0.0 {
                        sums[parent] += seconds;
                    }
                }
            }
        }
    }
    Ok(times)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(event_id: u32, parent: i64) -> Action {
        Action {
            event_id,
            action_id: 0,
            depth: 0,
            parent,
            flags: 0,
            marker_color_bits: None,
            marker_name: None,
            indices: 0,
            instances: 0,
            base_vertex: None,
            index_offset: None,
            vertex_offset: None,
            instance_offset: None,
            dispatch_groups: None,
            dispatch_threads: None,
            name: String::new(),
            api_events: Vec::new(),
        }
    }
    fn entry(event_id: u32, seconds: f64) -> CounterEntry {
        CounterEntry {
            event_id,
            counter: GPU_DURATION_COUNTER,
            bits: seconds.to_bits().to_string(),
        }
    }
    #[test]
    fn sums_nested_markers_and_skips_invalid_durations() {
        let actions = [
            action(9, -1),
            action(7, 0),
            action(3, 1),
            action(4, 1),
            action(5, 0),
        ];
        let values = [
            entry(3, 0.002),
            entry(4, f64::NAN),
            entry(5, 0.003),
            entry(7, 9.0),
        ];
        let times = aggregate(&actions, &values).unwrap();
        assert_eq!(times[&7], 0.002);
        assert_eq!(times[&9], 0.005);
        assert!(!times.contains_key(&4));
        assert!(
            aggregate(
                &actions,
                &[CounterEntry {
                    counter: 2,
                    ..entry(3, 1.0)
                }]
            )
            .is_err()
        );
    }
}
