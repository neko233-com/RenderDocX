use renderdocx::replay::Action;

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
    pub fn matches(&self, name: &str, eid: u32, action: Option<&Action>) -> bool {
        evaluate(&self.0, name, eid, action)
    }
}
fn evaluate(terms: &[Term], name: &str, eid: u32, action: Option<&Action>) -> bool {
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
            Expr::Text(text) => name.contains(text),
            Expr::Group(terms) => evaluate(terms, name, eid, action),
            Expr::Event(op, value) => op.test(i64::from(eid), *value),
            Expr::Action(condition) => action.is_some_and(|a| {
                a.flags & 0x80 == 0
                    && condition.as_ref().is_none_or(|(property, op, value)| {
                        action_property(a, property).is_some_and(|actual| op.test(actual, *value))
                    })
            }),
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
                    while self.peek().is_some_and(|c| c != ')') {
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
                            ) {
                                return Err(());
                            }
                            Expr::Action(Some((prop, op, n)))
                        }
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
