use crate::replay::Action;
use std::collections::{HashMap, HashSet};

/// One upstream `$param(name: value)` predicate. Names remain case-sensitive.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParamQuery {
    pub name: String,
    pub value: String,
}

/// A complete set of matching event IDs for each parameter predicate.
pub type ParameterMatches = HashMap<ParamQuery, HashSet<u32>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnnotationQuery {
    pub key: String,
    pub condition: AnnotationCondition,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnnotationCondition {
    Exists,
    Compare(String, String),
    Contains(String),
    Regex(String),
}
pub type AnnotationMatches = HashMap<AnnotationQuery, HashSet<u32>>;

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Must,
    Exclude,
}
#[derive(Clone, Copy)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
impl Op {
    fn test(self, a: i64, b: i64) -> bool {
        match self {
            Self::Eq => a == b,
            Self::Ne => a != b,
            Self::Lt => a < b,
            Self::Le => a <= b,
            Self::Gt => a > b,
            Self::Ge => a >= b,
        }
    }
}
enum Expr {
    Text(String),
    Group(Vec<Term>),
    Event(Op, i64),
    Action(Option<(String, Op, i64)>),
    ActionFlags(u32),
    Dispatch(Option<(String, Op, i64)>),
    Regex(pcre2::bytes::Regex),
    Parent(String),
    Param(ParamQuery),
    Annot(AnnotationQuery),
    Duration {
        dispatch_only: bool,
        op: Op,
        nanos: i64,
    },
}
struct Term {
    mode: Mode,
    expr: Expr,
}
#[derive(Default)]
pub struct Filter(Vec<Term>);
impl Filter {
    pub fn parse(input: &str) -> Result<Self, ()> {
        let chars: Vec<char> = input.chars().collect();
        Ok(Self(
            Parser {
                chars: &chars,
                pos: 0,
            }
            .terms(false, 0)?,
        ))
    }
    pub fn matches(&self, context: &EventContext<'_>) -> bool {
        evaluate(&self.0, context)
    }
    pub fn parameter_queries(&self) -> HashSet<ParamQuery> {
        fn collect(terms: &[Term], queries: &mut HashSet<ParamQuery>) {
            for term in terms {
                match &term.expr {
                    Expr::Param(query) => {
                        queries.insert(query.clone());
                    }
                    Expr::Group(group) => collect(group, queries),
                    _ => {}
                }
            }
        }
        let mut queries = HashSet::new();
        collect(&self.0, &mut queries);
        queries
    }
    pub fn annotation_queries(&self) -> HashSet<AnnotationQuery> {
        fn collect(terms: &[Term], queries: &mut HashSet<AnnotationQuery>) {
            for term in terms {
                match &term.expr {
                    Expr::Annot(query) => {
                        queries.insert(query.clone());
                    }
                    Expr::Group(group) => collect(group, queries),
                    _ => {}
                }
            }
        }
        let mut queries = HashSet::new();
        collect(&self.0, &mut queries);
        queries
    }
}
pub struct EventContext<'a> {
    pub name: &'a str,
    pub lower_name: &'a str,
    pub eid: u32,
    pub action: Option<&'a Action>,
    pub parent: Option<usize>,
    pub parents: &'a [Option<usize>],
    pub marker_names: &'a [String],
    pub duration: Option<f64>,
    pub parameter_matches: &'a ParameterMatches,
    pub annotation_matches: &'a AnnotationMatches,
}
fn evaluate(terms: &[Term], context: &EventContext<'_>) -> bool {
    if terms.is_empty() {
        return true;
    }
    let has_must = terms.iter().any(|t| matches!(t.mode, Mode::Must));
    let mut accepted = has_must;
    for term in terms {
        if has_must && matches!(term.mode, Mode::Normal) {
            continue;
        }
        let matched = match &term.expr {
            Expr::Text(text) => context.lower_name.contains(text),
            Expr::Group(terms) => evaluate(terms, context),
            Expr::Event(op, value) => op.test(i64::from(context.eid), *value),
            Expr::Action(condition) => context.action.is_some_and(|a| {
                a.flags & 0x80 == 0
                    && condition.as_ref().is_none_or(|(property, op, value)| {
                        action_property(a, property).is_some_and(|actual| op.test(actual, *value))
                    })
            }),
            Expr::ActionFlags(mask) => context.action.is_some_and(|a| a.flags & mask != 0),
            Expr::Dispatch(condition) => context.action.is_some_and(|a| {
                a.flags & 0x4 != 0
                    && condition.as_ref().is_none_or(|(property, op, value)| {
                        action_property(a, property).is_some_and(|actual| op.test(actual, *value))
                    })
            }),
            Expr::Regex(regex) => regex.is_match(context.name.as_bytes()).unwrap_or(false),
            Expr::Parent(marker) => {
                let mut parent = context.parent;
                let mut found = false;
                while let Some(index) = parent {
                    let Some(name) = context.marker_names.get(index) else {
                        break;
                    };
                    if name.contains(marker) {
                        found = true;
                        break;
                    }
                    let next = context.parents.get(index).copied().flatten();
                    if next.is_some_and(|next| next >= index) {
                        break;
                    }
                    parent = next;
                }
                found
            }
            Expr::Param(query) => context
                .parameter_matches
                .get(query)
                .is_some_and(|events| events.contains(&context.eid)),
            Expr::Annot(query) => context
                .annotation_matches
                .get(query)
                .is_some_and(|events| events.contains(&context.eid)),
            Expr::Duration {
                dispatch_only,
                op,
                nanos,
            } => {
                (!dispatch_only || context.action.is_some_and(|a| a.flags & 0x4 != 0))
                    && context.duration.is_some_and(|seconds| {
                        seconds.is_finite()
                            && seconds >= 0.0
                            && op.test((seconds * 1_000_000_000.0) as i64, *nanos)
                    })
            }
        };
        match term.mode {
            Mode::Normal => accepted |= matched,
            Mode::Must if !matched => return false,
            Mode::Exclude if matched => return false,
            _ => {}
        }
    }
    accepted
}
fn action_property(a: &Action, property: &str) -> Option<i64> {
    Some(match property {
        "eid" | "eventid" => a.event_id.into(),
        "actionid" => a.action_id.into(),
        "numindices" | "numindexes" | "numvertices" | "numvertexes" | "indexcount"
        | "vertexcount" => a.indices.into(),
        "numinstances" | "instancecount" => a.instances.into(),
        "basevertex" => a.base_vertex?.into(),
        "indexoffset" => a.index_offset?.into(),
        "vertexoffset" => a.vertex_offset?.into(),
        "instanceoffset" => a.instance_offset?.into(),
        "dispatchx" => a.dispatch_groups?[0].into(),
        "dispatchy" => a.dispatch_groups?[1].into(),
        "dispatchz" => a.dispatch_groups?[2].into(),
        "dispatchsize" => {
            let [x, y, z] = a.dispatch_groups?;
            x.wrapping_mul(y).wrapping_mul(z).into()
        }
        _ => return None,
    })
}
struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}
impl Parser<'_> {
    fn terms(&mut self, nested: bool, depth: usize) -> Result<Vec<Term>, ()> {
        if depth > 32 {
            return Err(());
        }
        let mut terms = Vec::new();
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.pos += 1;
            }
            if self.peek().is_none() {
                return if nested { Err(()) } else { Ok(terms) };
            }
            if self.peek() == Some(')') {
                if !nested || terms.is_empty() {
                    return Err(());
                }
                self.pos += 1;
                return Ok(terms);
            }
            let mut mode = Mode::Normal;
            while let Some(c @ ('+' | '-')) = self.peek() {
                mode = if c == '+' { Mode::Must } else { Mode::Exclude };
                self.pos += 1;
                while self.peek().is_some_and(char::is_whitespace) {
                    self.pos += 1;
                }
            }
            let expr = match self.peek().ok_or(())? {
                '(' => {
                    self.pos += 1;
                    Expr::Group(self.terms(true, depth + 1)?)
                }
                '"' => {
                    self.pos += 1;
                    let mut value = String::new();
                    loop {
                        let c = self.peek().ok_or(())?;
                        self.pos += 1;
                        if c == '"' {
                            break;
                        }
                        if c == '\\' {
                            let escaped = self.peek().ok_or(())?;
                            self.pos += 1;
                            value.push(escaped);
                        } else {
                            value.push(c);
                        }
                    }
                    Expr::Text(value.to_lowercase())
                }
                '$' => {
                    self.pos += 1;
                    let start = self.pos;
                    while self
                        .peek()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '.')
                    {
                        self.pos += 1;
                    }
                    let name: String = self.chars[start..self.pos].iter().collect();
                    while self.peek().is_some_and(char::is_whitespace) {
                        self.pos += 1;
                    }
                    if self.peek() != Some('(') {
                        return Err(());
                    }
                    self.pos += 1;
                    let start = self.pos;
                    let mut paren_depth = 1usize;
                    while let Some(c) = self.peek() {
                        if c == '(' {
                            paren_depth += 1;
                        } else if c == ')' {
                            paren_depth -= 1;
                            if paren_depth == 0 {
                                break;
                            }
                        }
                        self.pos += 1;
                    }
                    if self.peek() != Some(')') {
                        return Err(());
                    }
                    let params: String = self.chars[start..self.pos].iter().collect();
                    self.pos += 1;
                    match name.as_str() {
                        "event" => {
                            let (prop, op, n) = comparison(&params)?;
                            if prop != "eid" && prop != "eventid"
                                || !(0..=i64::from(u32::MAX)).contains(&n)
                            {
                                return Err(());
                            }
                            Expr::Event(op, n)
                        }
                        "action" if params.trim().is_empty() => Expr::Action(None),
                        "action"
                            if params
                                .trim_start()
                                .to_ascii_lowercase()
                                .starts_with("duration") =>
                        {
                            let (op, nanos) = duration_comparison(&params)?;
                            Expr::Duration {
                                dispatch_only: false,
                                op,
                                nanos,
                            }
                        }
                        "action"
                            if params
                                .trim_start()
                                .to_ascii_lowercase()
                                .starts_with("flags") =>
                        {
                            Expr::ActionFlags(parse_flags(&params)?)
                        }
                        "action" => {
                            let (prop, op, n) = comparison(&params)?;
                            if !matches!(
                                prop.as_str(),
                                "eid"
                                    | "eventid"
                                    | "actionid"
                                    | "numindices"
                                    | "numindexes"
                                    | "numvertices"
                                    | "numvertexes"
                                    | "indexcount"
                                    | "vertexcount"
                                    | "numinstances"
                                    | "instancecount"
                                    | "basevertex"
                                    | "indexoffset"
                                    | "vertexoffset"
                                    | "instanceoffset"
                                    | "dispatchx"
                                    | "dispatchy"
                                    | "dispatchz"
                                    | "dispatchsize"
                            ) {
                                return Err(());
                            }
                            Expr::Action(Some((prop, op, n)))
                        }
                        "dispatch" if params.trim().is_empty() => Expr::Dispatch(None),
                        "dispatch"
                            if params
                                .trim_start()
                                .to_ascii_lowercase()
                                .starts_with("duration") =>
                        {
                            let (op, nanos) = duration_comparison(&params)?;
                            Expr::Duration {
                                dispatch_only: true,
                                op,
                                nanos,
                            }
                        }
                        "dispatch" => {
                            let (prop, op, n) = comparison(&params)?;
                            let prop = match prop.as_str() {
                                "x" => "dispatchx".to_owned(),
                                "y" => "dispatchy".to_owned(),
                                "z" => "dispatchz".to_owned(),
                                "size" => "dispatchsize".to_owned(),
                                "eid" | "eventid" | "actionid" | "dispatchx" | "dispatchy"
                                | "dispatchz" | "dispatchsize" => prop,
                                _ => return Err(()),
                            };
                            Expr::Dispatch(Some((prop, op, n)))
                        }
                        "regex" => Expr::Regex(parse_regex(&params)?),
                        "param" => {
                            // Upstream prefers ':' even if '=' appears earlier.
                            let pos = params.find(':').or_else(|| params.find('=')).ok_or(())?;
                            let value = params[pos + 1..].trim();
                            if value.is_empty() {
                                return Err(());
                            }
                            Expr::Param(ParamQuery {
                                name: params[..pos].trim().to_owned(),
                                value: value.to_lowercase(),
                            })
                        }
                        "annot" => Expr::Annot(parse_annotation(&params)?),
                        "parent" | "childOf" => Expr::Parent(params.trim().to_lowercase()),
                        _ => return Err(()),
                    }
                }
                ')' => return Err(()),
                _ => {
                    let start = self.pos;
                    while self.peek().is_some_and(|c| !c.is_whitespace() && c != ')') {
                        if self.peek() == Some('(') {
                            return Err(());
                        }
                        self.pos += 1;
                    }
                    Expr::Text(
                        self.chars[start..self.pos]
                            .iter()
                            .collect::<String>()
                            .to_lowercase(),
                    )
                }
            };
            if self
                .peek()
                .is_some_and(|c| !c.is_whitespace() && !(nested && c == ')'))
            {
                return Err(());
            }
            terms.push(Term { mode, expr });
        }
    }
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
}
fn parse_annotation(input: &str) -> Result<AnnotationQuery, ()> {
    let input = input.trim();
    if input.is_empty() {
        return Err(());
    }
    let (key, rest) = if let Some(quoted) = input.strip_prefix('"') {
        let mut escaped = false;
        let mut end = None;
        for (index, c) in quoted.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
            } else if c == '"' {
                end = Some(index);
                break;
            }
        }
        let end = end.ok_or(())?;
        (
            quoted[..end].replace("\\\"", "\"").replace("\\\\", "\\"),
            quoted[end + 1..].trim(),
        )
    } else {
        let end = input
            .find(|c: char| c.is_whitespace() || "<>=~!".contains(c))
            .unwrap_or(input.len());
        (input[..end].to_owned(), input[end..].trim())
    };
    if key.is_empty() {
        return Err(());
    }
    let condition = if rest.is_empty() {
        AnnotationCondition::Exists
    } else if let Some(value) = rest
        .strip_prefix("contains")
        .filter(|value| value.starts_with(char::is_whitespace))
    {
        let value = value.trim();
        if value.is_empty() {
            return Err(());
        }
        AnnotationCondition::Contains(value.to_lowercase())
    } else {
        let op = ["==", "!=", "<=", ">=", "=~", "=", "<", ">"]
            .into_iter()
            .find(|op| rest.starts_with(op))
            .ok_or(())?;
        let value = rest[op.len()..].trim();
        if value.is_empty() {
            return Err(());
        }
        if op == "=~" {
            parse_regex(value)?;
            AnnotationCondition::Regex(value.to_owned())
        } else {
            AnnotationCondition::Compare(op.to_owned(), value.to_owned())
        }
    };
    Ok(AnnotationQuery { key, condition })
}
fn parse_flags(input: &str) -> Result<u32, ()> {
    let pos = input.find(['&', '=']).ok_or(())?;
    if !input[..pos].trim().eq_ignore_ascii_case("flags") {
        return Err(());
    }
    let mut mask = 0;
    let mut any = false;
    for token in input[pos + 1..].split_whitespace() {
        for flag in token.split('|') {
            let bit = match flag.to_ascii_lowercase().as_str() {
                "noflags" => 0,
                "clear" => 0x1,
                "drawcall" => 0x2,
                "dispatch" => 0x4,
                "meshdispatch" => 0x8,
                "cmdlist" => 0x10,
                "setmarker" => 0x20,
                "pushmarker" => 0x40,
                "popmarker" => 0x80,
                "present" => 0x100,
                "multiaction" => 0x200,
                "copy" => 0x400,
                "resolve" => 0x800,
                "genmips" => 0x1000,
                "passboundary" => 0x2000,
                "dispatchray" => 0x4000,
                "buildaccstruct" => 0x8000,
                "indexed" => 0x10000,
                "instanced" => 0x20000,
                "auto" => 0x40000,
                "indirect" => 0x80000,
                "clearcolor" => 0x100000,
                "cleardepthstencil" => 0x200000,
                "beginpass" => 0x400000,
                "endpass" => 0x800000,
                "commandbufferboundary" => 0x1000000,
                _ => return Err(()),
            };
            mask |= bit;
            any = true;
        }
    }
    if any { Ok(mask) } else { Err(()) }
}
pub(crate) fn parse_regex(input: &str) -> Result<pcre2::bytes::Regex, ()> {
    let input = input.trim();
    let body = input.strip_prefix('/').ok_or(())?;
    let mut escaped = false;
    let mut end = None;
    for (index, ch) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '/' {
            end = Some(index);
            break;
        }
    }
    let end = end.ok_or(())?;
    let mut builder = pcre2::bytes::RegexBuilder::new();
    builder.utf(true);
    for flag in body[end + 1..].chars() {
        match flag {
            // The checked-in upstream validates these options but does not
            // pass its PatternOptions to QRegularExpression.
            'i' | 'x' | 'u' => {}
            _ => return Err(()),
        }
    }
    builder.build(&body[..end]).map_err(|_| ())
}
fn duration_comparison(input: &str) -> Result<(Op, i64), ()> {
    let pos = input.find(['=', '!', '<', '>']).ok_or(())?;
    if !input[..pos].trim().eq_ignore_ascii_case("duration") {
        return Err(());
    }
    let rest = &input[pos..];
    let (op, rest) = [("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)]
        .into_iter()
        .find_map(|(prefix, op)| rest.strip_prefix(prefix).map(|r| (op, r)))
        .ok_or(())?;
    let value = rest.trim();
    let (number, scale) = [
        ("ms", 1_000_000.0),
        ("us", 1_000.0),
        ("ns", 1.0),
        ("s", 1_000_000_000.0),
    ]
    .into_iter()
    .find_map(|(suffix, scale)| value.strip_suffix(suffix).map(|v| (v, scale)))
    .ok_or(())?;
    let nanos = number.parse::<f64>().map_err(|_| ())? * scale;
    if !nanos.is_finite() || nanos < i64::MIN as f64 || nanos >= -(i64::MIN as f64) {
        return Err(());
    }
    Ok((op, nanos as i64))
}

fn comparison(input: &str) -> Result<(String, Op, i64), ()> {
    let pos = input.find(['=', '!', '<', '>']).ok_or(())?;
    let property = input[..pos].trim().to_lowercase();
    let rest = &input[pos..];
    let (op, rest) = [
        ("==", Op::Eq),
        ("!=", Op::Ne),
        ("<=", Op::Le),
        (">=", Op::Ge),
        ("=", Op::Eq),
        ("<", Op::Lt),
        (">", Op::Gt),
    ]
    .into_iter()
    .find_map(|(prefix, op)| rest.strip_prefix(prefix).map(|r| (op, r)))
    .ok_or(())?;
    let value = rest.trim();
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |s| (true, s));
    let (radix, digits) = if let Some(s) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (16, s)
    } else if value.len() > 1 && value.starts_with('0') {
        (8, &value[1..])
    } else {
        (10, value)
    };
    let magnitude = i64::from_str_radix(digits, radix).map_err(|_| ())?;
    Ok((property, op, if negative { -magnitude } else { magnitude }))
}
