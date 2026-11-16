//! Navigation over the ordered states returned by RenderDoc's shader debugger.
use crate::replay::{DebugState, DebugTrace, DebugVariable, InstructionSourceInfo};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

const DEBUG_BREAK: u32 = 0x4; // ShaderEvents::DebugBreak

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Breakpoint {
    Disassembly { line: u32 },
    Source { file_index: i32, line: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Step,
    Source,
    Breakpoint,
    Event,
    Boundary,
}

#[derive(Clone, Debug, Default)]
pub struct DebugNavigator {
    pub position: usize,
    pub breakpoints: BTreeSet<Breakpoint>,
    variables_cache: RefCell<Option<(usize, BTreeMap<String, DebugVariable>)>>,
}

fn location<'a>(trace: &'a DebugTrace, state: &DebugState) -> Option<&'a InstructionSourceInfo> {
    trace.instruction_info(state.next_instruction)
}

fn source_key(info: &InstructionSourceInfo) -> (i32, u32, u32, u32, u32) {
    (
        info.file_index,
        info.line_start,
        info.line_end,
        info.column_start,
        info.column_end,
    )
}

impl DebugNavigator {
    /// Reconstruct mutable variables from RenderDoc's ordered change stream.
    pub fn variables(&self, states: &[DebugState]) -> Vec<DebugVariable> {
        if states.is_empty() {
            return Vec::new();
        }
        let mut cache = self.variables_cache.borrow_mut();
        if cache.is_none() {
            let mut variables = BTreeMap::new();
            let first = &states[0];
            for change in &first.changes {
                if !change.after.name.is_empty() {
                    variables.insert(change.after.name.clone(), change.after.clone());
                }
            }
            *cache = Some((0, variables));
        }
        let (position, variables) = cache.as_mut().unwrap();
        let target = self.position.min(states.len() - 1);
        while *position < target {
            *position += 1;
            for change in &states[*position].changes {
                if change.after.name.is_empty() {
                    variables.remove(&change.before.name);
                } else {
                    variables.insert(change.after.name.clone(), change.after.clone());
                }
            }
        }
        while *position > target {
            for change in states[*position].changes.iter().rev() {
                if change.before.name.is_empty() {
                    variables.remove(&change.after.name);
                } else {
                    variables.insert(change.before.name.clone(), change.before.clone());
                }
            }
            *position -= 1;
        }
        variables.values().cloned().collect()
    }

    pub fn current<'a>(&self, states: &'a [DebugState]) -> Option<&'a DebugState> {
        states.get(self.position)
    }

    pub fn set_breakpoint(
        &mut self,
        trace: &DebugTrace,
        breakpoint: Breakpoint,
        enabled: bool,
    ) -> Result<(), &'static str> {
        let valid = trace.instructions.iter().any(|info| match breakpoint {
            Breakpoint::Disassembly { line } => line > 0 && info.disassembly_line == line,
            Breakpoint::Source { file_index, line } => {
                file_index >= 0
                    && line > 0
                    && info.file_index == file_index
                    && info.line_start == line
            }
        });
        if !valid {
            return Err("Breakpoint does not match an instruction location");
        }
        if enabled {
            self.breakpoints.insert(breakpoint);
        } else {
            self.breakpoints.remove(&breakpoint);
        }
        Ok(())
    }

    pub fn current_breakpoint(
        &self,
        trace: &DebugTrace,
        states: &[DebugState],
        source: bool,
    ) -> Option<Breakpoint> {
        let info = location(trace, self.current(states)?)?;
        if source && info.file_index >= 0 && info.line_start > 0 {
            Some(Breakpoint::Source {
                file_index: info.file_index,
                line: info.line_start,
            })
        } else if !source && info.disassembly_line > 0 {
            Some(Breakpoint::Disassembly {
                line: info.disassembly_line,
            })
        } else {
            None
        }
    }

    fn hit(&self, trace: &DebugTrace, state: &DebugState) -> bool {
        location(trace, state).is_some_and(|info| {
            (info.disassembly_line > 0
                && self.breakpoints.contains(&Breakpoint::Disassembly {
                    line: info.disassembly_line,
                }))
                || (info.file_index >= 0
                    && info.line_start > 0
                    && self.breakpoints.contains(&Breakpoint::Source {
                        file_index: info.file_index,
                        line: info.line_start,
                    }))
        })
    }

    fn advance(&mut self, states: &[DebugState], forward: bool) -> bool {
        if self.current(states).is_none() {
            return false;
        }
        let next = if forward {
            self.position.checked_add(1).filter(|&v| v < states.len())
        } else {
            self.position.checked_sub(1)
        };
        if let Some(next) = next {
            self.position = next;
            true
        } else {
            false
        }
    }

    pub fn step(&mut self, states: &[DebugState], forward: bool) -> StopReason {
        if self.advance(states, forward) {
            StopReason::Step
        } else {
            StopReason::Boundary
        }
    }

    /// Source step into: stop at a changed mapped source location, a breakpoint, or a boundary.
    /// If the starting state has no source location, move one instruction.
    pub fn step_source(
        &mut self,
        trace: &DebugTrace,
        states: &[DebugState],
        forward: bool,
    ) -> StopReason {
        let old = self
            .current(states)
            .and_then(|state| location(trace, state))
            .filter(|info| info.file_index >= 0 && info.line_start > 0)
            .map(source_key);
        if old.is_none() {
            return self.step(states, forward);
        }
        while self.advance(states, forward) {
            let state = &states[self.position];
            if self.hit(trace, state) {
                return StopReason::Breakpoint;
            }
            if location(trace, state).is_some_and(|info| {
                info.file_index >= 0 && info.line_start > 0 && Some(source_key(info)) != old
            }) {
                return StopReason::Source;
            }
        }
        StopReason::Boundary
    }

    /// Run past the current state and stop at a breakpoint, selected ShaderEvents, or an end.
    pub fn run(
        &mut self,
        trace: &DebugTrace,
        states: &[DebugState],
        forward: bool,
        event_mask: u32,
    ) -> StopReason {
        // A source line can map to several consecutive instructions. Resume past
        // the current source breakpoint before allowing that breakpoint to fire again.
        let mut suppressed_source = self
            .current_breakpoint(trace, states, true)
            .filter(|point| self.breakpoints.contains(point));
        while self.advance(states, forward) {
            let state = &states[self.position];
            if state.flags & (event_mask | DEBUG_BREAK) != 0 {
                return StopReason::Event;
            }
            if let Some(info) = location(trace, state) {
                let source = Breakpoint::Source {
                    file_index: info.file_index,
                    line: info.line_start,
                };
                if suppressed_source != Some(source) {
                    suppressed_source = None;
                }
                if (info.disassembly_line > 0
                    && self.breakpoints.contains(&Breakpoint::Disassembly {
                        line: info.disassembly_line,
                    }))
                    || (info.file_index >= 0
                        && info.line_start > 0
                        && suppressed_source != Some(source)
                        && self.breakpoints.contains(&source))
                {
                    return StopReason::Breakpoint;
                }
            }
        }
        StopReason::Boundary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DebugTrace, Vec<DebugState>) {
        let trace = serde_json::from_value(serde_json::json!({"event_id":1,"stage":5,"inputs":[],"constant_blocks":[],"read_only_resources":[],"read_write_resources":[],"samplers":[],"instructions":[
            {"instruction":0,"disassembly_line":1,"file_index":0,"line_start":3,"line_end":3,"column_start":0,"column_end":0,"source_variables":[]},
            {"instruction":2,"disassembly_line":3,"file_index":0,"line_start":4,"line_end":4,"column_start":0,"column_end":0,"source_variables":[]},
            {"instruction":3,"disassembly_line":4,"file_index":-1,"line_start":0,"line_end":0,"column_start":0,"column_end":0,"source_variables":[]},
            {"instruction":4,"disassembly_line":5,"file_index":0,"line_start":5,"line_end":5,"column_start":0,"column_end":0,"source_variables":[]}
        ]})).unwrap();
        let states = (0..5)
            .map(|i| DebugState {
                step: i,
                next_instruction: i,
                flags: if i == 3 { 2 } else { 0 },
                callstack: vec![],
                changes: vec![],
            })
            .collect();
        (trace, states)
    }

    #[test]
    fn reconstructs_variables_while_navigating_both_directions() {
        let (_, mut states) = fixture();
        let var = |name: &str, value: &str| DebugVariable {
            name: name.into(),
            rows: 1,
            columns: 1,
            variable_type: 0,
            flags: 0,
            value_hex: value.into(),
            members: vec![],
        };
        let change = |before, after| crate::replay::DebugChange { before, after };
        states[0].changes = vec![change(var("", ""), var("a", "01"))];
        states[1].changes = vec![
            change(var("a", "01"), var("a", "02")),
            change(var("", ""), var("b", "03")),
        ];
        states[2].changes = vec![change(var("a", "02"), var("", ""))];
        let mut nav = DebugNavigator::default();
        let values = |nav: &DebugNavigator| {
            nav.variables(&states)
                .into_iter()
                .map(|v| (v.name, v.value_hex))
                .collect::<Vec<_>>()
        };
        assert_eq!(values(&nav), vec![("a".into(), "01".into())]);
        nav.step(&states, true);
        assert_eq!(
            values(&nav),
            vec![("a".into(), "02".into()), ("b".into(), "03".into())]
        );
        nav.step(&states, true);
        assert_eq!(values(&nav), vec![("b".into(), "03".into())]);
        nav.step(&states, false);
        assert_eq!(
            values(&nav),
            vec![("a".into(), "02".into()), ("b".into(), "03".into())]
        );
        nav.step(&states, false);
        assert_eq!(values(&nav), vec![("a".into(), "01".into())]);
    }

    #[test]
    fn stepping_and_breakpoints_work_in_both_directions() {
        let (trace, states) = fixture();
        let mut nav = DebugNavigator::default();
        assert_eq!(nav.step(&states, false), StopReason::Boundary);
        assert_eq!(nav.step_source(&trace, &states, true), StopReason::Source);
        assert_eq!(nav.position, 2);
        let bp = Breakpoint::Source {
            file_index: 0,
            line: 5,
        };
        nav.set_breakpoint(&trace, bp, true).unwrap();
        assert_eq!(nav.run(&trace, &states, true, 2), StopReason::Event);
        assert_eq!(nav.position, 3);
        assert_eq!(nav.run(&trace, &states, true, 0), StopReason::Breakpoint);
        assert_eq!(nav.position, 4);
        assert_eq!(nav.step_source(&trace, &states, false), StopReason::Source);
        assert_eq!(nav.position, 2);
        assert!(
            nav.set_breakpoint(&trace, Breakpoint::Disassembly { line: 99 }, true)
                .is_err()
        );
        assert_eq!(nav.step(&[], true), StopReason::Boundary);
    }

    #[test]
    fn resuming_a_source_breakpoint_skips_same_line_until_reentered() {
        let (trace, mut states) = fixture();
        states.push(DebugState {
            step: 5,
            next_instruction: 1,
            flags: 0,
            callstack: vec![],
            changes: vec![],
        });
        let mut nav = DebugNavigator::default();
        nav.set_breakpoint(
            &trace,
            Breakpoint::Source {
                file_index: 0,
                line: 3,
            },
            true,
        )
        .unwrap();
        assert_eq!(nav.run(&trace, &states, true, 0), StopReason::Breakpoint);
        assert_eq!(nav.position, 5);
        assert_eq!(nav.run(&trace, &states, false, 0), StopReason::Breakpoint);
        assert_eq!(nav.position, 1);
    }

    #[test]
    fn instruction_breakpoints_and_upstream_debug_break_events_stop_run() {
        let (trace, mut states) = fixture();
        let mut nav = DebugNavigator::default();
        nav.set_breakpoint(&trace, Breakpoint::Disassembly { line: 3 }, true)
            .unwrap();
        assert_eq!(nav.run(&trace, &states, true, 0), StopReason::Breakpoint);
        assert_eq!(nav.position, 2);
        nav.set_breakpoint(&trace, Breakpoint::Disassembly { line: 3 }, false)
            .unwrap();
        states[3].flags = 4;
        assert_eq!(nav.run(&trace, &states, true, 0), StopReason::Event);
        assert_eq!(nav.position, 3);
    }
}
